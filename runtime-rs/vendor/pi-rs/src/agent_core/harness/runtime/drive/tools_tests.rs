//! Behavior tests for [`super::run_tools`], ported from
//! `pi/packages/agent/test/harness/runtime/drive-tools.test.ts`
//! (SHA256 5a6dcdd3662154d574aaa87e1865eb161c860a2ead466c7515f68499b86c03de)
//! as the behavior authority, plus a serialization-seam oracle test whose
//! expected JSON strings were captured by running the upstream pure functions
//! (`syntheticMessage`/`abortedOutcome`/`interruptedOutcome`/
//! `truncatedOutcome` and the `publishToolIntent`/`publishToolOutcome` event
//! literals) with node `--experimental-strip-types`
//! (`tests/fixtures/tools_oracle/tools_oracle.ts`, timestamp stubbed to 1234).
//!
//! Ported substitutions: `ObservedMemoryStorage` becomes a wrapping
//! [`ObservedStorage`] (the Rust `MemoryStorage` is not subclassable); the
//! transcript order assertion reads `find_entries` oldest-first; the late
//! `setMemo` rejection assertion checks the memo stays absent and `getMemo`
//! resolves `None` (the invocation trait surface cannot reject).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::future::BoxFuture;

use super::*;
use crate::agent_core::harness::hooks::{BeforeToolHookResult, HookHandler};
use crate::agent_core::harness::runtime::drive_pass::DriveOptions;
use crate::agent_core::harness::runtime::lane::{EmitBatch, RuntimeConfig};
use crate::agent_core::harness::runtime::restore::restore_lane;
use crate::agent_core::harness::session::types::{AscDescOrder, EntryQuery, ValueWrite};
use crate::agent_core::harness::session::values::operation_tool_args_prefix;
use crate::agent_core::harness::session::{
    self as session, MemoryStorage, MemoryStorageOptions, Session as _, SessionMetadata,
    SessionReader as _, Storage, StorageBackedSession,
};
use crate::agent_core::harness::session::{LaneConfiguration, LaneModel, Write};
use crate::agent_core::harness::types::AgentHarnessToolUpdateOptions;
use crate::agent_core::harness::{background_context, DEFAULT_COMPACTION_SETTINGS};
use crate::agent_core::types::{QueueMode, ThinkingLevel};
use crate::ai::models::faux::{faux_provider, FauxProviderOptions};
use crate::ai::models::{create_models, CreateModelsOptions};

type Observations = Arc<Mutex<Vec<String>>>;
type CallStatesFn = Box<dyn FnOnce(&[String]) -> Vec<ToolCall>>;
type ExtraWritesFn = Box<dyn FnOnce(&ExtraWrites) -> Vec<Write>>;
type OnToolEvent = Arc<dyn Fn(&ToolEvent) -> BoxFuture<'static, ()> + Send + Sync>;

const SCHEMA: &str =
    r#"{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}"#;

// --- observed storage (upstream ObservedMemoryStorage) ---

struct ObservedStorage {
    inner: MemoryStorage,
    observations: Observations,
}

fn classify_commit(writes: &[Write]) -> Option<&'static str> {
    let intents = writes.iter().any(|write| {
        matches!(
            write,
            Write::Value(ValueWrite::Set { namespace, .. }) if namespace == "pi.op.tool_args"
        )
    });
    if intents {
        return Some("intent_commit");
    }
    let stages_outcome = writes.iter().any(|write| {
        matches!(
            write,
            Write::Value(ValueWrite::Set { namespace, .. }) if namespace == "pi.pending.entry"
        )
    });
    if stages_outcome {
        return Some("outcome_commit");
    }
    let replay = writes.iter().any(|write| {
        matches!(
            write,
            Write::Value(ValueWrite::Delete { namespace, .. })
                if namespace == "pi.pending.tool_output"
        )
    });
    if replay {
        return Some("replay_commit");
    }
    None
}

impl Storage for ObservedStorage {
    fn commit<'a>(
        &'a self,
        writes: Vec<Write>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<session::CommitResult>> {
        if let Some(kind) = classify_commit(&writes) {
            self.observations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(kind.to_owned());
        }
        self.inner.commit(writes, context)
    }

    fn get_entries<'a>(
        &'a self,
        ids: &[String],
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<std::collections::HashMap<String, session::Entry>>> {
        self.inner.get_entries(ids, context)
    }

    fn get_value<'a>(
        &'a self,
        address: &session::ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Option<session::StoredValue>>> {
        self.inner.get_value(address, context)
    }

    fn scan_values<'a>(
        &'a self,
        prefix: &session::ValueAddress,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<session::StoredValue>>> {
        self.inner.scan_values(prefix, context)
    }

    fn read_list<'a>(
        &'a self,
        address: &session::ValueAddress,
        options: Option<&session::ListReadOptions>,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<session::ListElement>>> {
        self.inner.read_list(address, options, context)
    }

    fn scan_branch<'a>(
        &'a self,
        query: &session::StorageBranchScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<session::Entry>>> {
        self.inner.scan_branch(query, context)
    }

    fn scan_branch_structure<'a>(
        &'a self,
        query: &session::StorageBranchScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<session::EntryStructure>>> {
        self.inner.scan_branch_structure(query, context)
    }

    fn scan_entries<'a>(
        &'a self,
        query: &session::EntryScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<session::Entry>>> {
        self.inner.scan_entries(query, context)
    }

    fn scan_usage<'a>(
        &'a self,
        query: &session::UsageScan,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<Vec<session::UsageRow>>> {
        self.inner.scan_usage(query, context)
    }

    fn get_stats<'a>(
        &'a self,
        context: Context,
    ) -> BoxFuture<'a, anyhow::Result<session::SessionStats>> {
        self.inner.get_stats(context)
    }

    fn close<'a>(&'a self, context: Context) -> BoxFuture<'a, anyhow::Result<()>> {
        self.inner.close(context)
    }
}

// --- fixture ---

struct Fixture {
    session: Arc<StorageBackedSession>,
    lane: Arc<Lane>,
    drive: Arc<Drive>,
    tool_events: Arc<Mutex<Vec<ToolEvent>>>,
    harness_events: Arc<Mutex<Vec<HarnessEvent>>>,
    assistant_entry_id: String,
    result_entry_ids: Vec<String>,
    operation_id: String,
    observations: Observations,
    tools: Vec<AgentHarnessTool<()>>,
    tool_context: ToolContextSource<()>,
    tool_emit: ToolEventEmit,
}

struct FixtureOptions {
    /// `(tool name, argument value)` per source index.
    calls: Vec<(&'static str, &'static str)>,
    tools: Vec<AgentHarnessTool<()>>,
    mode: ToolExecutionMode,
    stop_reason: &'static str,
    call_states: Option<CallStatesFn>,
    extra_writes: Option<ExtraWritesFn>,
    /// When set, the tool context resolves through a counting provider
    /// (upstream `toolContext: () => undefined`).
    context_resolutions: Option<Arc<AtomicUsize>>,
    cancelled: bool,
    /// Blocking hook invoked per tool event before its delivery resolves
    /// (upstream `onEmit` on the event batches).
    on_tool_event: Option<OnToolEvent>,
}

impl Default for FixtureOptions {
    fn default() -> Self {
        FixtureOptions {
            calls: Vec::new(),
            tools: Vec::new(),
            mode: ToolExecutionMode::Parallel,
            stop_reason: "toolUse",
            call_states: None,
            extra_writes: None,
            context_resolutions: None,
            cancelled: false,
            on_tool_event: None,
        }
    }
}

struct ExtraWrites {
    operation_id: String,
    result_entry_ids: Vec<String>,
}

fn harness_tool(
    name: &str,
    replay: Option<ToolReplay>,
    execute: impl Fn(
            String,
            serde_json::Value,
            Arc<crate::agent_core::harness::types::AgentHarnessToolUpdateCallback>,
            (),
            Arc<dyn AgentHarnessToolInvocation>,
            Context,
        ) -> BoxFuture<'static, anyhow::Result<AgentToolResult>>
        + Send
        + Sync
        + 'static,
) -> AgentHarnessTool<()> {
    AgentHarnessTool {
        name: name.to_owned(),
        label: name.to_owned(),
        description: name.to_owned(),
        parameters: serde_json::from_str(SCHEMA).unwrap(),
        constrained_sampling: None,
        execute: Arc::new(execute),
        prepare_arguments: None,
        replay,
        execution_mode: None,
    }
}

fn text_result(text: &str, details: serde_json::Value) -> AgentToolResult {
    AgentToolResult {
        content: vec![text_block(text)],
        details: Some(details),
        usage: None,
        terminate: None,
    }
}

fn tool_event_type(event: &ToolEvent) -> &'static str {
    match event {
        ToolEvent::ToolStart { .. } => "tool_start",
        ToolEvent::ToolUpdate { .. } => "tool_update",
        ToolEvent::ToolEnd { .. } => "tool_end",
    }
}

fn harness_event_type(event: &HarnessEvent) -> String {
    serde_json::to_value(event).expect("harness event serializes")["type"]
        .as_str()
        .expect("typed event")
        .to_owned()
}

async fn create_fixture(options: FixtureOptions) -> Fixture {
    let observations: Observations = Arc::new(Mutex::new(Vec::new()));
    let storage = Arc::new(ObservedStorage {
        inner: MemoryStorage::new(MemoryStorageOptions {
            now: Some(Arc::new(|| 100)),
        }),
        observations: Arc::clone(&observations),
    });
    let sess = Arc::new(StorageBackedSession::new(
        SessionMetadata {
            id: "drive-tools-test".into(),
            created_at: 1,
            storage_version: 1,
            ..Default::default()
        },
        storage,
    ));
    let operation_id = sess.id_generator().next(Some(10));
    let assistant_entry_id = sess.id_generator().next(Some(20));
    let result_entry_ids: Vec<String> = options
        .calls
        .iter()
        .map(|_| sess.id_generator().next(Some(20)))
        .collect();
    let configuration = LaneConfiguration {
        model: LaneModel {
            provider: "faux".to_owned(),
            model_id: "faux-1".to_owned(),
        },
        thinking_level: ThinkingLevel::Off,
        active_tool_names: options
            .calls
            .iter()
            .map(|(name, _)| name.to_string())
            .collect(),
    };
    let blocks: Vec<serde_json::Value> = options
        .calls
        .iter()
        .enumerate()
        .map(|(index, (name, value))| {
            serde_json::json!({
                "type": "toolCall",
                "id": format!("call-{index}"),
                "name": name,
                "arguments": {"value": value},
            })
        })
        .collect();
    let assistant: AgentMessage = serde_json::from_value(serde_json::json!({
        "role": "assistant",
        "content": blocks,
        "api": "faux",
        "provider": "faux",
        "model": "faux-1",
        "stopReason": options.stop_reason,
        "usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0,
            "totalTokens": 2, "cost": {"input": 0, "output": 0, "cacheRead": 0,
            "cacheWrite": 0, "total": 0}},
        "timestamp": 20,
    }))
    .unwrap();
    let calls = match options.call_states {
        Some(build) => build(&result_entry_ids),
        None => result_entry_ids
            .iter()
            .enumerate()
            .map(|(index, result_entry_id)| ToolCall {
                source_index: index,
                result_entry_id: result_entry_id.clone(),
                state: ToolCallState::Planned,
            })
            .collect(),
    };
    let control = if options.cancelled {
        Control::CancelRequested { requested_at: 30 }
    } else {
        Control::Running
    };
    let run = OperationState {
        scope: crate::agent_core::harness::runtime::durable::OperationScope {
            control,
            settings: crate::agent_core::harness::runtime::durable::RunSettings {
                compaction: DEFAULT_COMPACTION_SETTINGS,
                steering_mode: QueueMode::All,
                follow_up_mode: QueueMode::All,
                tool_execution: options.mode,
            },
            latest_assistant_entry_id: Some(assistant_entry_id.clone()),
        },
        phase: OperationPhase::Tools {
            batch: ToolBatch {
                assistant_entry_id: assistant_entry_id.clone(),
                configuration: configuration.clone(),
                turn_id: "turn-1".to_owned(),
                calls,
            },
        },
    };
    let mut writes: Vec<Write> = vec![
        session::insert_entry(session::NewEntry::Message {
            id: assistant_entry_id.clone(),
            parent_id: None,
            message: assistant,
            terminate: None,
        }),
        session::set_value(
            &session::branch_tip("main"),
            serde_json::Value::String(assistant_entry_id.clone()),
        ),
        session::set_value(
            &session::lane_config("main"),
            session::lane_configuration_value(&configuration),
        ),
        session::set_value(
            &session::lane_state("main"),
            serde_json::json!({"currentOperationId": operation_id, "lastOperationId": null, "inbox": []}),
        ),
        session::set_value(
            &session::operation_meta(&operation_id),
            serde_json::json!({
                "operationId": operation_id,
                "lane": "main",
                "sourceTipId": null,
                "startedAt": 10,
                "intent": {"kind": "run", "promptEntryIds": []},
            }),
        ),
        session::set_value(
            &session::operation_state(&operation_id),
            serde_json::to_value(&run).unwrap(),
        ),
    ];
    if let Some(extra_writes) = options.extra_writes {
        let extra = ExtraWrites {
            operation_id: operation_id.clone(),
            result_entry_ids: result_entry_ids.clone(),
        };
        writes.extend(extra_writes(&extra));
    }
    sess.mutate(
        move |reader, context| {
            Box::pin(async move { reader.commit(writes, context).await.map(|_| ()) })
        },
        background_context(),
    )
    .await
    .unwrap();

    let faux = faux_provider(FauxProviderOptions::default());
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));

    let harness_events: Arc<Mutex<Vec<HarnessEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let emit: EmitBatch = {
        let harness_events = Arc::clone(&harness_events);
        let observations = Arc::clone(&observations);
        Arc::new(move |events, _context| {
            let harness_events = Arc::clone(&harness_events);
            let observations = Arc::clone(&observations);
            Box::pin(async move {
                for event in &events {
                    observations
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(harness_event_type(event));
                    harness_events
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(event.clone());
                }
                Ok(())
            })
        })
    };
    let tool_events: Arc<Mutex<Vec<ToolEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let tool_emit: ToolEventEmit = {
        let tool_events = Arc::clone(&tool_events);
        let observations = Arc::clone(&observations);
        let on_tool_event = options.on_tool_event;
        Arc::new(move |events, _context| {
            let tool_events = Arc::clone(&tool_events);
            let observations = Arc::clone(&observations);
            let on_tool_event = on_tool_event.clone();
            Box::pin(async move {
                for event in &events {
                    observations
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(tool_event_type(event).to_owned());
                    tool_events
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(event.clone());
                    if let Some(on_tool_event) = &on_tool_event {
                        on_tool_event(event).await;
                    }
                }
                Ok(())
            })
        })
    };

    let state = restore_lane(sess.as_ref(), "main", background_context())
        .await
        .expect("lane restores");
    let lane = Lane::new(
        "main",
        Arc::clone(&sess),
        models,
        crate::agent_core::harness::hooks::HookRegistry::new(Arc::new(
            |_error: anyhow::Error,
             _hook: crate::agent_core::harness::hooks::HookName,
             _message: String,
             _context| { Box::pin(async {}) },
        )),
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
            resources: Default::default(),
            stream_options: Default::default(),
            steering_mode: QueueMode::All,
            follow_up_mode: QueueMode::All,
            tool_execution: options.mode,
            entry_projectors: None,
        }),
    );
    let drive = Arc::new(Drive::new(
        &DriveOptions {
            operation_id: operation_id.clone(),
            wait_for_retry: None,
            poll_deferred: None,
        },
        background_context(),
    ));
    let tool_context = match options.context_resolutions {
        Some(resolutions) => ToolContextSource::Provider(Arc::new(move |_context| {
            resolutions.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {}) as BoxFuture<'static, ()>
        })),
        None => ToolContextSource::Value(()),
    };
    Fixture {
        session: sess,
        lane,
        drive,
        tool_events,
        harness_events,
        assistant_entry_id,
        result_entry_ids,
        operation_id,
        observations,
        tools: options.tools,
        tool_context,
        tool_emit,
    }
}

fn current_run(fixture: &Fixture) -> OperationState {
    fixture
        .lane
        .state()
        .operation
        .expect("fixture has an operation")
        .state
}

fn current_calls(fixture: &Fixture) -> Vec<ToolCall> {
    match current_run(fixture).phase {
        OperationPhase::Tools { batch } => batch.calls,
        _ => Vec::new(),
    }
}

async fn drive_tools(fixture: &Fixture) -> anyhow::Result<ProcedureResult> {
    let run = current_run(fixture);
    run_tools(
        &fixture.lane,
        &fixture.drive,
        &run,
        &fixture.tools,
        &fixture.tool_context,
        &fixture.tool_emit,
    )
    .await
}

/// Drive on a separate task so the test can interact while tools run.
fn spawn_drive_tools(
    fixture: &Fixture,
) -> tokio::task::JoinHandle<anyhow::Result<ProcedureResult>> {
    let lane = Arc::clone(&fixture.lane);
    let drive = Arc::clone(&fixture.drive);
    let run = current_run(fixture);
    let tools = fixture.tools.clone();
    let tool_context = fixture.tool_context.clone();
    let tool_emit = Arc::clone(&fixture.tool_emit);
    tokio::spawn(
        async move { run_tools(&lane, &drive, &run, &tools, &tool_context, &tool_emit).await },
    )
}

async fn expect_projection_restores(fixture: &Fixture) {
    let restored = restore_lane(fixture.session.as_ref(), "main", background_context())
        .await
        .expect("projection restores");
    assert_eq!(restored, fixture.lane.state());
}

async fn wait_for(predicate: impl Fn() -> bool) {
    for _ in 0..2_000 {
        if predicate() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    panic!("condition was not reached");
}

async fn transcript_ids(fixture: &Fixture) -> Vec<String> {
    let entries = fixture
        .session
        .find_entries(
            Some(&EntryQuery {
                order: Some(AscDescOrder::Asc),
                ..Default::default()
            }),
            background_context(),
        )
        .await
        .unwrap();
    entries
        .into_iter()
        .map(|entry| entry.id().to_owned())
        .collect()
}

fn observations(fixture: &Fixture) -> Vec<String> {
    fixture
        .observations
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn observation_index(fixture: &Fixture, needle: &str) -> usize {
    observations(fixture)
        .iter()
        .position(|entry| entry == needle)
        .unwrap_or_else(|| panic!("observation {needle} missing"))
}

// --- the ported drive-tools tests ---

#[tokio::test]
async fn executes_a_sequential_batch_with_memos_checkpoints_hooks_usage_and_source_order_placement()
{
    let usage_json = serde_json::json!({
        "input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 3,
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0},
    });
    let late_invocation: Arc<Mutex<Option<Arc<dyn AgentHarnessToolInvocation>>>> =
        Arc::new(Mutex::new(None));
    let tool_checks: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let first = harness_tool("first", Some(ToolReplay::Never), {
        let late_invocation = Arc::clone(&late_invocation);
        let tool_checks = Arc::clone(&tool_checks);
        let usage_json = usage_json.clone();
        move |_tool_call_id, args, on_update, _tool_context, invocation, _context| {
            let late_invocation = Arc::clone(&late_invocation);
            let tool_checks = Arc::clone(&tool_checks);
            let usage_json = usage_json.clone();
            Box::pin(async move {
                *late_invocation.lock().unwrap() = Some(Arc::clone(&invocation));
                invocation
                    .set_memo("step/a", Some(serde_json::json!({"value": "memo"})))
                    .await;
                let memo = invocation.get_memo("step/a").await;
                tool_checks.lock().unwrap().push(format!(
                    "memo_ok:{}",
                    memo == Some(serde_json::json!({"value": "memo"}))
                ));
                on_update(
                    &AgentToolResult {
                        content: vec![text_block("partial")],
                        details: Some(serde_json::json!({"progress": "partial"})),
                        usage: None,
                        terminate: None,
                    },
                    AgentHarnessToolUpdateOptions { checkpoint: true },
                );
                let value = args["value"].as_str().unwrap_or_default().to_owned();
                Ok(AgentToolResult {
                    content: vec![text_block(&value)],
                    details: Some(serde_json::json!({"value": value})),
                    usage: Some(serde_json::from_value(usage_json).unwrap()),
                    terminate: None,
                })
            })
        }
    });
    let second = harness_tool("second", Some(ToolReplay::Never), {
        move |_tool_call_id, args, _on_update, _tool_context, _invocation, _context| {
            Box::pin(async move {
                let value = args["value"].as_str().unwrap_or_default().to_owned();
                Ok(text_result(&value, serde_json::json!({"value": value})))
            })
        }
    });
    let context_resolutions = Arc::new(AtomicUsize::new(0));
    let fixture = create_fixture(FixtureOptions {
        calls: vec![("first", "first"), ("second", "second")],
        tools: vec![first, second],
        mode: ToolExecutionMode::Sequential,
        context_resolutions: Some(Arc::clone(&context_resolutions)),
        ..Default::default()
    })
    .await;
    let checkpoint_seen = Arc::new(AtomicBool::new(false));
    // The after_tool hook observes the still-present checkpoint for `first`.
    let session_holder = Arc::new(Mutex::new(Some(Arc::clone(&fixture.session))));
    let operation_holder = Arc::new(Mutex::new(Some(fixture.operation_id.clone())));
    let entry_holder = Arc::new(Mutex::new(Some(fixture.result_entry_ids[0].clone())));
    let seen = Arc::clone(&checkpoint_seen);
    let before_tool: HookHandler = Arc::new(|invocation, _context| {
        Box::pin(async move {
            match &invocation.event {
                crate::agent_core::harness::hooks::HookEvent::BeforeTool(event)
                    if event.tool_name == "first" =>
                {
                    let mut args = event.args.clone();
                    args["value"] = serde_json::json!("prepared");
                    Ok(HookResult::BeforeTool(Some(BeforeToolHookResult {
                        args: Some(args),
                        block: None,
                    })))
                }
                _ => Ok(HookResult::BeforeTool(None)),
            }
        })
    });
    let after_tool: HookHandler = Arc::new(move |invocation, context| {
        let session = session_holder.lock().unwrap().clone();
        let operation_id = operation_holder.lock().unwrap().clone();
        let result_entry_id = entry_holder.lock().unwrap().clone();
        let seen = Arc::clone(&seen);
        Box::pin(async move {
            if let (
                Some(session),
                Some(operation_id),
                Some(result_entry_id),
                crate::agent_core::harness::hooks::HookEvent::AfterTool(event),
            ) = (session, operation_id, result_entry_id, &invocation.event)
            {
                if event.tool_name == "first" {
                    let stored = session
                        .get_value(
                            &pending_tool_output(&operation_id, &result_entry_id),
                            context,
                        )
                        .await?;
                    seen.store(stored.is_some(), Ordering::SeqCst);
                }
            }
            Ok(HookResult::AfterTool(None))
        })
    });
    fixture
        .lane
        .hooks()
        .on(
            crate::agent_core::harness::hooks::HookName::BeforeTool,
            before_tool,
            None,
        )
        .unwrap();
    fixture
        .lane
        .hooks()
        .on(
            crate::agent_core::harness::hooks::HookName::AfterTool,
            after_tool,
            None,
        )
        .unwrap();

    let outcome = drive_tools(&fixture).await.unwrap();
    assert_eq!(outcome, ProcedureResult::Continue);
    let ids = transcript_ids(&fixture).await;
    assert_eq!(
        ids,
        vec![
            fixture.assistant_entry_id.clone(),
            fixture.result_entry_ids[0].clone(),
            fixture.result_entry_ids[1].clone()
        ]
    );
    let first_entry = fixture
        .session
        .get_entry(&fixture.result_entry_ids[0], background_context())
        .await
        .unwrap()
        .expect("first result entry");
    match &first_entry {
        session::Entry::Message { message, .. } => match message {
            AgentMessage::ToolResult(message) => {
                let crate::ai::types::message::TextOrImageBlock::Text(text) = &message.content[0]
                else {
                    panic!("expected a text block");
                };
                assert_eq!(text.text, "prepared");
            }
            _ => panic!("expected a toolResult message"),
        },
        _ => panic!("expected a message entry"),
    }
    assert_eq!(context_resolutions.load(Ordering::SeqCst), 1);
    assert!(
        checkpoint_seen.load(Ordering::SeqCst),
        "after_tool saw the checkpoint"
    );
    assert!(
        tool_checks
            .lock()
            .unwrap()
            .iter()
            .any(|check| check == "memo_ok:true"),
        "memo round-trips during execution: {:?}",
        tool_checks.lock().unwrap()
    );
    let run = current_run(&fixture);
    match &run.phase {
        OperationPhase::Checkpoint { checkpoint } => {
            assert_eq!(
                checkpoint.continuation,
                crate::agent_core::harness::runtime::durable::Continuation::NeedAssistant {
                    overflow_recovery_used: false
                }
            );
        }
        other => panic!("expected a checkpoint phase, got {other:?}"),
    }
    assert_eq!(
        fixture.lane.state().configuration.active_tool_names,
        vec!["first".to_owned(), "second".to_owned()]
    );
    assert!(
        fixture
            .session
            .scan_values(
                &operation_tool_args_prefix(&fixture.operation_id, None),
                background_context(),
            )
            .await
            .unwrap()
            .is_empty(),
        "tool arguments are cleaned up"
    );
    assert!(
        fixture
            .session
            .scan_values(
                &operation_tool_memo_prefix(&fixture.operation_id, None),
                background_context(),
            )
            .await
            .unwrap()
            .is_empty(),
        "memos are cleaned up"
    );
    assert!(
        fixture
            .session
            .get_value(
                &pending_tool_output(&fixture.operation_id, &fixture.result_entry_ids[0]),
                background_context(),
            )
            .await
            .unwrap()
            .is_none(),
        "checkpoint is cleaned up"
    );
    // A late memo access no longer owns the durable effect.
    let late = late_invocation
        .lock()
        .unwrap()
        .clone()
        .expect("invocation captured");
    late.set_memo("late", Some(serde_json::json!(true))).await;
    assert!(
        fixture
            .session
            .get_value(
                &operation_tool_memo(&fixture.operation_id, &fixture.result_entry_ids[0], "late"),
                background_context(),
            )
            .await
            .unwrap()
            .is_none(),
        "late setMemo must not store anything"
    );
    assert_eq!(
        late.get_memo("late").await,
        None,
        "late getMemo resolves absent"
    );
    // Event ordering through the observations log.
    let entries = observations(&fixture);
    let index = |needle: &str| {
        entries
            .iter()
            .position(|entry| entry == needle)
            .unwrap_or_else(|| panic!("observation {needle} missing"))
    };
    let intent = index("intent_commit");
    let start = index("tool_start");
    let update = index("tool_update");
    let commit = index("outcome_commit");
    let end = index("tool_end");
    let placement = index("entry_added");
    assert!(intent < start);
    assert!(start < update);
    assert!(update < commit);
    assert!(commit < end);
    assert!(end < placement);
    let harness = fixture.harness_events.lock().unwrap().clone();
    assert_eq!(
        harness
            .iter()
            .filter(|event| harness_event_type(event) == "entry_added")
            .count(),
        2
    );
    assert_eq!(
        harness
            .iter()
            .filter(|event| harness_event_type(event) == "usage")
            .count(),
        1
    );
    assert!(matches!(harness.last(), Some(HarnessEvent::TurnEnd { .. })));
    expect_projection_restores(&fixture).await;
}

#[tokio::test]
async fn stages_parallel_completion_order_but_materializes_source_order() {
    let (finish_a_tx, finish_a_rx) = tokio::sync::watch::channel(false);
    let (finish_b_tx, finish_b_rx) = tokio::sync::watch::channel(false);
    let started: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let make = |name: &'static str, finish: tokio::sync::watch::Receiver<bool>| {
        let started = Arc::clone(&started);
        harness_tool(
            name,
            Some(ToolReplay::Never),
            move |_id, _args, _on, _ctx, _inv, _c| {
                let started = Arc::clone(&started);
                let mut finish = finish.clone();
                Box::pin(async move {
                    started.lock().unwrap().push(name.to_owned());
                    let _ = finish.wait_for(|done| *done).await;
                    Ok(text_result(name, serde_json::json!({"value": name})))
                })
            },
        )
    };
    let fixture = create_fixture(FixtureOptions {
        calls: vec![("a", "a"), ("b", "b")],
        tools: vec![make("a", finish_a_rx), make("b", finish_b_rx)],
        mode: ToolExecutionMode::Parallel,
        ..Default::default()
    })
    .await;
    let running = spawn_drive_tools(&fixture);
    wait_for(|| started.lock().unwrap().len() == 2).await;
    finish_b_tx.send_replace(true);
    wait_for(|| {
        current_calls(&fixture)
            .get(1)
            .is_some_and(|call| matches!(call.state, ToolCallState::OutcomeReady { .. }))
    })
    .await;
    let statuses: Vec<String> = current_calls(&fixture)
        .iter()
        .map(|call| match call.state {
            ToolCallState::Planned => "planned".to_owned(),
            ToolCallState::EffectPending { .. } => "effect_pending".to_owned(),
            ToolCallState::OutcomeReady { .. } => "outcome_ready".to_owned(),
            ToolCallState::Completed { .. } => "completed".to_owned(),
        })
        .collect();
    assert_eq!(statuses, vec!["effect_pending", "outcome_ready"]);
    assert!(
        fixture
            .session
            .get_entry(&fixture.result_entry_ids[1], background_context())
            .await
            .unwrap()
            .is_none(),
        "the settled outcome is staged before it is placed"
    );
    let events = fixture.tool_events.lock().unwrap().clone();
    let start_b = events
        .iter()
        .find_map(|event| match event {
            ToolEvent::ToolStart {
                tool_name, args, ..
            } if tool_name == "b" => Some(args.clone()),
            _ => None,
        })
        .expect("tool_start for b");
    assert_eq!(start_b, serde_json::json!({"value": "b"}));
    let end_b = events
        .iter()
        .find_map(|event| match event {
            ToolEvent::ToolEnd {
                tool_name, result, ..
            } if tool_name == "b" => Some(result.clone()),
            _ => None,
        })
        .expect("tool_end for b");
    assert_eq!(
        serde_json::to_value(&end_b.content).unwrap(),
        serde_json::json!([{"type": "text", "text": "b"}])
    );
    finish_a_tx.send_replace(true);
    running.await.unwrap().unwrap();

    let ids = transcript_ids(&fixture).await;
    assert_eq!(
        ids,
        vec![
            fixture.assistant_entry_id.clone(),
            fixture.result_entry_ids[0].clone(),
            fixture.result_entry_ids[1].clone(),
        ]
    );
    expect_projection_restores(&fixture).await;
}

#[tokio::test]
async fn safe_replays_persisted_arguments_and_memos_while_interrupting_unsafe_effects() {
    let safe_checks: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let unsafe_called = Arc::new(AtomicBool::new(false));
    let safe = harness_tool("safe", Some(ToolReplay::Safe), {
        let safe_checks = Arc::clone(&safe_checks);
        move |_id, args, _on, _ctx, invocation, _c| {
            let safe_checks = Arc::clone(&safe_checks);
            Box::pin(async move {
                safe_checks
                    .lock()
                    .unwrap()
                    .push(format!("invocation:{}", invocation.invocation_id()));
                let memo = invocation.get_memo("step/a").await;
                safe_checks.lock().unwrap().push(format!(
                    "memo_ok:{}",
                    memo == Some(serde_json::json!({"complete": true}))
                ));
                safe_checks.lock().unwrap().push(format!(
                    "args:{}",
                    args["value"].as_str().unwrap_or_default()
                ));
                Ok(text_result(
                    "safe replay",
                    serde_json::json!({"value": "safe"}),
                ))
            })
        }
    });
    let never = harness_tool("unsafe", Some(ToolReplay::Never), {
        let unsafe_called = Arc::clone(&unsafe_called);
        move |_id, _args, _on, _ctx, _inv, _c| {
            let unsafe_called = Arc::clone(&unsafe_called);
            Box::pin(async move {
                unsafe_called.store(true, Ordering::SeqCst);
                Ok(text_result("must not run", serde_json::json!({})))
            })
        }
    });
    let fixture = create_fixture(FixtureOptions {
        calls: vec![("safe", "safe"), ("unsafe", "unsafe")],
        tools: vec![safe, never],
        call_states: Some(Box::new(|ids| {
            vec![
                ToolCall {
                    source_index: 0,
                    result_entry_id: ids[0].clone(),
                    state: ToolCallState::EffectPending { replay: ToolReplay::Safe },
                },
                ToolCall {
                    source_index: 1,
                    result_entry_id: ids[1].clone(),
                    state: ToolCallState::EffectPending { replay: ToolReplay::Never },
                },
            ]
        })),
        extra_writes: Some(Box::new(|extra| {
            let operation_id = extra.operation_id.clone();
            let ids = extra.result_entry_ids.clone();
            vec![
                session::set_value(
                    &operation_tool_args(&operation_id, "turn-1", 0),
                    serde_json::json!({"value": "persisted"}),
                ),
                session::set_value(
                    &operation_tool_args(&operation_id, "turn-1", 1),
                    serde_json::json!({"value": "unsafe"}),
                ),
                session::set_value(
                    &operation_tool_memo(&operation_id, &ids[0], "step/a"),
                    serde_json::json!({"complete": true}),
                ),
                session::set_value(
                    &pending_tool_output(&operation_id, &ids[0]),
                    serde_json::json!({"content": [{"type": "text", "text": "old progress"}], "details": {}}),
                ),
                session::set_value(
                    &pending_tool_output(&operation_id, &ids[1]),
                    serde_json::json!({
                        "content": [{"type": "text", "text": "durable partial"}],
                        "details": {"progress": "kept"},
                    }),
                ),
            ]
        })),
        ..Default::default()
    })
    .await;

    drive_tools(&fixture).await.unwrap();
    let checks = safe_checks.lock().unwrap().clone();
    assert_eq!(
        checks
            .iter()
            .filter(|check| check.starts_with("invocation:"))
            .count(),
        1,
        "safe executed exactly once"
    );
    assert!(
        checks.iter().any(|check| check == "memo_ok:true"),
        "safe read the persisted memo: {checks:?}"
    );
    assert!(
        checks.iter().any(|check| check == "args:persisted"),
        "safe replayed the persisted arguments: {checks:?}"
    );
    assert!(!unsafe_called.load(Ordering::SeqCst), "unsafe must not run");

    let events = fixture.tool_events.lock().unwrap().clone();
    let safe_start = events
        .iter()
        .find_map(|event| match event {
            ToolEvent::ToolStart {
                tool_call_id,
                args,
                recovery,
                ..
            } if tool_call_id == "call-0" => Some((args.clone(), *recovery)),
            _ => None,
        })
        .expect("tool_start for the safe call");
    assert_eq!(safe_start.0, serde_json::json!({"value": "persisted"}));
    assert!(safe_start.1, "safe replay start carries recovery");
    assert!(
        observation_index(&fixture, "replay_commit") < observation_index(&fixture, "tool_start"),
        "the replay commit lands before its tool_start delivery"
    );
    let safe_end = events
        .iter()
        .find_map(|event| match event {
            ToolEvent::ToolEnd {
                tool_call_id,
                is_error,
                recovery,
                ..
            } if tool_call_id == "call-0" => Some((*is_error, *recovery)),
            _ => None,
        })
        .expect("tool_end for the safe call");
    assert!(!safe_end.0);
    assert!(safe_end.1);

    let unsafe_entry = fixture
        .session
        .get_entry(&fixture.result_entry_ids[1], background_context())
        .await
        .unwrap()
        .expect("unsafe result entry");
    match &unsafe_entry {
        session::Entry::Message { message, .. } => match message {
            AgentMessage::ToolResult(message) => {
                assert!(message.is_error);
                assert_eq!(
                    message.details,
                    Some(serde_json::json!({"progress": "kept"}))
                );
                let crate::ai::types::message::TextOrImageBlock::Text(text) =
                    message.content.last().unwrap()
                else {
                    panic!("expected a text block");
                };
                assert!(text.text.contains("external outcome is unknown"));
            }
            _ => panic!("expected a toolResult message"),
        },
        _ => panic!("expected a message entry"),
    }
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ToolEvent::ToolStart { tool_call_id, .. } if tool_call_id == "call-1")),
        "interrupted unsafe effects get no tool_start"
    );
    let unsafe_end = events
        .iter()
        .find_map(|event| match event {
            ToolEvent::ToolEnd {
                tool_call_id,
                is_error,
                recovery,
                ..
            } if tool_call_id == "call-1" => Some((*is_error, *recovery)),
            _ => None,
        })
        .expect("tool_end for the unsafe call");
    assert!(unsafe_end.0);
    assert!(unsafe_end.1);
}

#[tokio::test]
async fn reconciles_a_restored_cancelled_batch_without_hooks_context_or_effects() {
    let execute_called = Arc::new(AtomicBool::new(false));
    let execute = harness_tool("planned", Some(ToolReplay::Never), {
        let execute_called = Arc::clone(&execute_called);
        move |_id, _args, _on, _ctx, _inv, _c| {
            let execute_called = Arc::clone(&execute_called);
            Box::pin(async move {
                execute_called.store(true, Ordering::SeqCst);
                Ok(AgentToolResult::default())
            })
        }
    });
    let pending = harness_tool("pending", Some(ToolReplay::Safe), {
        let execute_called = Arc::clone(&execute_called);
        move |_id, _args, _on, _ctx, _inv, _c| {
            let execute_called = Arc::clone(&execute_called);
            Box::pin(async move {
                execute_called.store(true, Ordering::SeqCst);
                Ok(AgentToolResult::default())
            })
        }
    });
    let context_resolutions = Arc::new(AtomicUsize::new(0));
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let fixture = create_fixture(FixtureOptions {
        calls: vec![("planned", "planned"), ("pending", "pending")],
        tools: vec![execute, pending],
        context_resolutions: Some(Arc::clone(&context_resolutions)),
        cancelled: true,
        call_states: Some(Box::new(|ids| {
            vec![
                ToolCall {
                    source_index: 0,
                    result_entry_id: ids[0].clone(),
                    state: ToolCallState::Planned,
                },
                ToolCall {
                    source_index: 1,
                    result_entry_id: ids[1].clone(),
                    state: ToolCallState::EffectPending { replay: ToolReplay::Safe },
                },
            ]
        })),
        extra_writes: Some(Box::new(|extra| {
            vec![
                session::set_value(
                    &operation_tool_args(&extra.operation_id, "turn-1", 1),
                    serde_json::json!({"value": "pending"}),
                ),
                session::set_value(
                    &pending_tool_output(&extra.operation_id, &extra.result_entry_ids[1]),
                    serde_json::json!({"content": [{"type": "text", "text": "checkpoint"}], "details": {}}),
                ),
            ]
        })),
        ..Default::default()
    })
    .await;
    let hook_recorder = Arc::clone(&hook_calls);
    let noop_hook: HookHandler = Arc::new(move |_invocation, _context| {
        let hook_recorder = Arc::clone(&hook_recorder);
        Box::pin(async move {
            hook_recorder.fetch_add(1, Ordering::SeqCst);
            Ok(HookResult::BeforeTool(None))
        })
    });
    fixture
        .lane
        .hooks()
        .on(
            crate::agent_core::harness::hooks::HookName::BeforeTool,
            noop_hook,
            None,
        )
        .unwrap();

    drive_tools(&fixture).await.unwrap();
    assert!(!execute_called.load(Ordering::SeqCst), "no effect runs");
    assert_eq!(
        context_resolutions.load(Ordering::SeqCst),
        0,
        "context unresolved"
    );
    assert_eq!(hook_calls.load(Ordering::SeqCst), 0, "hooks skipped");
    let run = current_run(&fixture);
    assert!(matches!(
        run.scope.control,
        crate::agent_core::harness::session::Control::CancelRequested { .. }
    ));
    match &run.phase {
        OperationPhase::Checkpoint { checkpoint } => {
            assert_eq!(
                checkpoint.continuation,
                crate::agent_core::harness::runtime::durable::Continuation::NeedAssistant {
                    overflow_recovery_used: false
                }
            );
        }
        other => panic!("expected a checkpoint phase, got {other:?}"),
    }
    let pending_entry = fixture
        .session
        .get_entry(&fixture.result_entry_ids[1], background_context())
        .await
        .unwrap()
        .expect("pending result entry");
    match &pending_entry {
        session::Entry::Message { message, .. } => match message {
            AgentMessage::ToolResult(message) => {
                let crate::ai::types::message::TextOrImageBlock::Text(text) =
                    message.content.last().unwrap()
                else {
                    panic!("expected a text block");
                };
                assert!(text.text.contains("external outcome is unknown"));
            }
            _ => panic!("expected a toolResult message"),
        },
        _ => panic!("expected a message entry"),
    }
    let events = fixture.tool_events.lock().unwrap().clone();
    let starts: Vec<&ToolEvent> = events
        .iter()
        .filter(|event| matches!(event, ToolEvent::ToolStart { .. }))
        .collect();
    let ends: Vec<&ToolEvent> = events
        .iter()
        .filter(|event| matches!(event, ToolEvent::ToolEnd { .. }))
        .collect();
    assert_eq!(starts.len(), 1);
    match starts[0] {
        ToolEvent::ToolStart {
            tool_name, args, ..
        } => {
            assert_eq!(tool_name, "planned");
            assert_eq!(args, &serde_json::json!({"value": "planned"}));
        }
        _ => unreachable!(),
    }
    assert_eq!(ends.len(), 2);
    let start_call_id = match starts[0] {
        ToolEvent::ToolStart { tool_call_id, .. } => tool_call_id.clone(),
        _ => unreachable!(),
    };
    let aborted_end = ends
        .iter()
        .find_map(|event| match event {
            ToolEvent::ToolEnd {
                tool_call_id,
                is_error,
                ..
            } if tool_call_id == &start_call_id => Some(*is_error),
            _ => None,
        })
        .expect("the aborted planned call ends");
    assert!(aborted_end);
}

#[tokio::test]
async fn materializes_outcome_ready_state_without_resolving_tools_or_tool_context() {
    let execute_called = Arc::new(AtomicBool::new(false));
    let execute = harness_tool("ready", Some(ToolReplay::Never), {
        let execute_called = Arc::clone(&execute_called);
        move |_id, _args, _on, _ctx, _inv, _c| {
            let execute_called = Arc::clone(&execute_called);
            Box::pin(async move {
                execute_called.store(true, Ordering::SeqCst);
                Ok(AgentToolResult::default())
            })
        }
    });
    let context_resolutions = Arc::new(AtomicUsize::new(0));
    let fixture = create_fixture(FixtureOptions {
        calls: vec![("ready", "ready")],
        tools: vec![execute],
        context_resolutions: Some(Arc::clone(&context_resolutions)),
        call_states: Some(Box::new(|ids| {
            vec![ToolCall {
                source_index: 0,
                result_entry_id: ids[0].clone(),
                state: ToolCallState::OutcomeReady { terminate: true },
            }]
        })),
        extra_writes: Some(Box::new(|extra| {
            let staged: AgentMessage = serde_json::from_value(serde_json::json!({
                "role": "toolResult",
                "toolCallId": "call-0",
                "toolName": "ready",
                "content": [{"type": "text", "text": "already done"}],
                "isError": false,
                "timestamp": 30,
            }))
            .unwrap();
            vec![session::set_value(
                &pending_entry(&extra.result_entry_ids[0]),
                serde_json::to_value(&staged).unwrap(),
            )]
        })),
        ..Default::default()
    })
    .await;

    drive_tools(&fixture).await.unwrap();
    assert!(!execute_called.load(Ordering::SeqCst));
    assert_eq!(context_resolutions.load(Ordering::SeqCst), 0);
    let run = current_run(&fixture);
    match &run.phase {
        OperationPhase::Checkpoint { checkpoint } => {
            assert_eq!(
                checkpoint.continuation,
                crate::agent_core::harness::runtime::durable::Continuation::MayFinish {
                    include_final_assistant: false
                }
            );
        }
        other => panic!("expected a checkpoint phase, got {other:?}"),
    }
    assert_eq!(
        fixture.lane.state().configuration.active_tool_names,
        vec!["ready".to_owned()]
    );
    let harness = fixture.harness_events.lock().unwrap().clone();
    assert!(matches!(
        harness.first(),
        Some(HarnessEvent::TurnStart { recovery: true, .. })
    ));
    assert!(matches!(
        harness.last(),
        Some(HarnessEvent::TurnEnd { recovery: true, .. })
    ));
    expect_projection_restores(&fixture).await;
}

#[tokio::test]
async fn never_executes_genuine_length_or_missing_tool_calls() {
    let execute_called = Arc::new(AtomicBool::new(false));
    let execute = harness_tool("present", Some(ToolReplay::Never), {
        let execute_called = Arc::clone(&execute_called);
        move |_id, _args, _on, _ctx, _inv, _c| {
            let execute_called = Arc::clone(&execute_called);
            Box::pin(async move {
                execute_called.store(true, Ordering::SeqCst);
                Ok(AgentToolResult::default())
            })
        }
    });
    let truncated = create_fixture(FixtureOptions {
        calls: vec![("present", "present")],
        tools: vec![execute],
        stop_reason: "length",
        ..Default::default()
    })
    .await;
    let truncated_after_tool_called = Arc::new(AtomicBool::new(false));
    let recorder = Arc::clone(&truncated_after_tool_called);
    let after_tool: HookHandler = Arc::new(move |_invocation, _context| {
        let recorder = Arc::clone(&recorder);
        Box::pin(async move {
            recorder.store(true, Ordering::SeqCst);
            Ok(HookResult::AfterTool(None))
        })
    });
    truncated
        .lane
        .hooks()
        .on(
            crate::agent_core::harness::hooks::HookName::AfterTool,
            after_tool,
            None,
        )
        .unwrap();
    drive_tools(&truncated).await.unwrap();
    assert!(!execute_called.load(Ordering::SeqCst));
    let entry = truncated
        .session
        .get_entry(&truncated.result_entry_ids[0], background_context())
        .await
        .unwrap()
        .expect("truncated result entry");
    match &entry {
        session::Entry::Message { message, .. } => match message {
            AgentMessage::ToolResult(message) => {
                let crate::ai::types::message::TextOrImageBlock::Text(text) = &message.content[0]
                else {
                    panic!("expected a text block");
                };
                assert!(text.text.contains("arguments may be truncated"));
            }
            _ => panic!("expected a toolResult message"),
        },
        _ => panic!("expected a message entry"),
    }
    assert!(!truncated_after_tool_called.load(Ordering::SeqCst));
    let events = truncated.tool_events.lock().unwrap().clone();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ToolEvent::ToolStart { .. }))
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ToolEvent::ToolEnd { .. }))
            .count(),
        1
    );
    assert!(
        observation_index(&truncated, "outcome_commit")
            < observation_index(&truncated, "tool_start")
    );
    assert!(
        observation_index(&truncated, "tool_start") < observation_index(&truncated, "tool_end")
    );
    assert!(
        observation_index(&truncated, "tool_end") < observation_index(&truncated, "entry_added")
    );

    let missing = create_fixture(FixtureOptions {
        calls: vec![("missing", "missing")],
        tools: Vec::new(),
        ..Default::default()
    })
    .await;
    let missing_after_tool_called = Arc::new(AtomicBool::new(false));
    let recorder = Arc::clone(&missing_after_tool_called);
    let after_tool: HookHandler = Arc::new(move |_invocation, _context| {
        let recorder = Arc::clone(&recorder);
        Box::pin(async move {
            recorder.store(true, Ordering::SeqCst);
            Ok(HookResult::AfterTool(None))
        })
    });
    missing
        .lane
        .hooks()
        .on(
            crate::agent_core::harness::hooks::HookName::AfterTool,
            after_tool,
            None,
        )
        .unwrap();
    drive_tools(&missing).await.unwrap();
    let entry = missing
        .session
        .get_entry(&missing.result_entry_ids[0], background_context())
        .await
        .unwrap()
        .expect("missing result entry");
    match &entry {
        session::Entry::Message { message, .. } => match message {
            AgentMessage::ToolResult(message) => {
                assert!(message.is_error);
                assert!(message.details.is_none(), "no details key");
                let crate::ai::types::message::TextOrImageBlock::Text(text) = &message.content[0]
                else {
                    panic!("expected a text block");
                };
                assert_eq!(text.text, "Tool \"missing\" is unavailable");
            }
            _ => panic!("expected a toolResult message"),
        },
        _ => panic!("expected a message entry"),
    }
    assert!(!missing_after_tool_called.load(Ordering::SeqCst));
    let events = missing.tool_events.lock().unwrap().clone();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ToolEvent::ToolStart { .. }))
            .count(),
        1
    );
    match events
        .iter()
        .find(|event| matches!(event, ToolEvent::ToolStart { .. }))
        .unwrap()
    {
        ToolEvent::ToolStart { args, .. } => {
            assert_eq!(args, &serde_json::json!({"value": "missing"}))
        }
        _ => unreachable!(),
    }
    let ends: Vec<&ToolEvent> = events
        .iter()
        .filter(|event| matches!(event, ToolEvent::ToolEnd { .. }))
        .collect();
    assert_eq!(ends.len(), 1);
    match ends[0] {
        ToolEvent::ToolEnd {
            result, is_error, ..
        } => {
            assert!(is_error);
            let serialized = serde_json::to_value(result).unwrap();
            assert_eq!(
                serialized["content"],
                serde_json::json!([{"type": "text", "text": "Tool \"missing\" is unavailable"}])
            );
            assert!(serialized.get("details").is_none());
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn awaits_update_delivery_and_checkpoint_persistence_before_after_tool() {
    let (update_queued_tx, update_queued_rx) = tokio::sync::watch::channel(false);
    let (release_update_tx, release_update_rx) = tokio::sync::watch::channel(false);
    let updating = harness_tool("updating", Some(ToolReplay::Never), {
        move |_id, _args, on_update, _ctx, _inv, _c| {
            Box::pin(async move {
                on_update(
                    &AgentToolResult {
                        content: vec![text_block("partial")],
                        details: Some(serde_json::json!({"progress": "partial"})),
                        usage: None,
                        terminate: None,
                    },
                    AgentHarnessToolUpdateOptions { checkpoint: true },
                );
                Ok(text_result("done", serde_json::json!({})))
            })
        }
    });
    let on_tool_event: OnToolEvent = {
        let update_queued_tx = update_queued_tx.clone();
        let release_update_rx = release_update_rx.clone();
        Arc::new(move |event: &ToolEvent| {
            let event = event.clone();
            let update_queued_tx = update_queued_tx.clone();
            let release_update_rx = release_update_rx.clone();
            Box::pin(async move {
                if matches!(event, ToolEvent::ToolUpdate { .. }) {
                    update_queued_tx.send_replace(true);
                    let _ = release_update_rx
                        .clone()
                        .wait_for(|released| *released)
                        .await;
                }
            })
        })
    };
    let fixture = create_fixture(FixtureOptions {
        calls: vec![("updating", "updating")],
        tools: vec![updating],
        on_tool_event: Some(on_tool_event),
        ..Default::default()
    })
    .await;
    let after_started = Arc::new(AtomicBool::new(false));
    let checkpoint_present = Arc::new(Mutex::new(None::<bool>));
    let session_holder = Arc::new(Mutex::new(Some(Arc::clone(&fixture.session))));
    let operation_holder = Arc::new(Mutex::new(Some(fixture.operation_id.clone())));
    let entry_holder = Arc::new(Mutex::new(Some(fixture.result_entry_ids[0].clone())));
    let after_flag = Arc::clone(&after_started);
    let checkpoint_holder = Arc::clone(&checkpoint_present);
    let after_tool: HookHandler = Arc::new(move |_invocation, context| {
        let session = session_holder.lock().unwrap().clone();
        let operation_id = operation_holder.lock().unwrap().clone();
        let result_entry_id = entry_holder.lock().unwrap().clone();
        let after_flag = Arc::clone(&after_flag);
        let checkpoint_holder = Arc::clone(&checkpoint_holder);
        Box::pin(async move {
            after_flag.store(true, Ordering::SeqCst);
            if let (Some(session), Some(operation_id), Some(result_entry_id)) =
                (session, operation_id, result_entry_id)
            {
                let stored = session
                    .get_value(
                        &pending_tool_output(&operation_id, &result_entry_id),
                        context,
                    )
                    .await?;
                *checkpoint_holder.lock().unwrap() = Some(stored.is_some());
            }
            Ok(HookResult::AfterTool(None))
        })
    });
    fixture
        .lane
        .hooks()
        .on(
            crate::agent_core::harness::hooks::HookName::AfterTool,
            after_tool,
            None,
        )
        .unwrap();

    let running = spawn_drive_tools(&fixture);
    let _ = update_queued_rx.clone().wait_for(|queued| *queued).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(
        !after_started.load(Ordering::SeqCst),
        "after_tool waits for the blocked update delivery"
    );
    release_update_tx.send_replace(true);
    running.await.unwrap().unwrap();
    assert!(after_started.load(Ordering::SeqCst));
    assert_eq!(
        *checkpoint_present.lock().unwrap(),
        Some(true),
        "the checkpoint is still present during after_tool"
    );
}

#[tokio::test]
async fn does_not_stage_a_cancelled_outcome_before_cancellation_is_durable() {
    let execute_called = Arc::new(AtomicBool::new(false));
    let execute = harness_tool("cancel-before-admission", Some(ToolReplay::Never), {
        let execute_called = Arc::clone(&execute_called);
        move |_id, _args, _on, _ctx, _inv, _c| {
            let execute_called = Arc::clone(&execute_called);
            Box::pin(async move {
                execute_called.store(true, Ordering::SeqCst);
                Ok(AgentToolResult::default())
            })
        }
    });
    let fixture = create_fixture(FixtureOptions {
        calls: vec![("cancel-before-admission", "cancel-before-admission")],
        tools: vec![execute],
        mode: ToolExecutionMode::Sequential,
        ..Default::default()
    })
    .await;
    let cancellation = tokio_util::sync::CancellationToken::new();
    fixture.drive.begin_abort(cancellation.clone());
    let running = spawn_drive_tools(&fixture);
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    assert!(matches!(
        current_calls(&fixture)[0].state,
        ToolCallState::Planned
    ));
    assert!(
        fixture
            .session
            .get_value(
                &pending_entry(&fixture.result_entry_ids[0]),
                background_context(),
            )
            .await
            .unwrap()
            .is_none(),
        "nothing staged before the abort refusal resolves"
    );
    commit_cancel_requested(&fixture, 40).await;
    cancellation.cancel();
    fixture.drive.signal_abort();
    running.await.unwrap().unwrap();

    assert!(!execute_called.load(Ordering::SeqCst));
    assert!(
        fixture
            .session
            .get_entry(&fixture.result_entry_ids[0], background_context())
            .await
            .unwrap()
            .is_some(),
        "the aborted outcome is staged"
    );
}

async fn commit_cancel_requested(fixture: &Fixture, requested_at: i64) {
    let operation_id = fixture.operation_id.clone();
    fixture
        .lane
        .command(
            move |state, _reader| {
                let operation_id = operation_id.clone();
                Box::pin(async move {
                    let Some(operation) = &state.operation else {
                        anyhow::bail!("missing operation");
                    };
                    let next_run = OperationState {
                        scope: crate::agent_core::harness::runtime::durable::OperationScope {
                            control: Control::CancelRequested { requested_at },
                            settings: operation.state.scope.settings.clone(),
                            latest_assistant_entry_id: operation
                                .state
                                .scope
                                .latest_assistant_entry_id
                                .clone(),
                        },
                        phase: operation.state.phase.clone(),
                    };
                    let mut next = state.clone();
                    next.operation =
                        Some(crate::agent_core::harness::runtime::durable::Operation {
                            meta: operation.meta.clone(),
                            state: next_run.clone(),
                        });
                    Ok(LaneCommand::Commit {
                        writes: vec![session::set_value(
                            &session::operation_state(&operation_id),
                            serde_json::to_value(&next_run)?,
                        )],
                        next,
                        materialize: Box::new(|_commit: &session::CommitResult| ()),
                        events: None,
                    })
                })
            },
            background_context(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn drains_live_updates_before_after_tool_and_stages_a_non_terminating_result_after_cancellation(
) {
    let (started_tx, started_rx) = tokio::sync::watch::channel(false);
    let (update_delivery_tx, update_delivery_rx) = tokio::sync::watch::channel(false);
    let slow = harness_tool("slow", Some(ToolReplay::Never), {
        move |_id, _args, on_update, _ctx, _inv, context| {
            let started_tx = started_tx.clone();
            Box::pin(async move {
                on_update(
                    &AgentToolResult {
                        content: vec![text_block("partial")],
                        details: Some(serde_json::json!({"progress": "partial"})),
                        usage: None,
                        terminate: None,
                    },
                    AgentHarnessToolUpdateOptions { checkpoint: false },
                );
                started_tx.send_replace(true);
                if let Some(signal) = context.abort_signal() {
                    signal.cancelled().await;
                }
                anyhow::bail!("cancelled effect");
            })
        }
    });
    let on_tool_event: OnToolEvent = {
        let update_delivery_rx = update_delivery_rx.clone();
        Arc::new(move |event: &ToolEvent| {
            let event = event.clone();
            let update_delivery_rx = update_delivery_rx.clone();
            Box::pin(async move {
                if matches!(event, ToolEvent::ToolUpdate { .. }) {
                    let _ = update_delivery_rx
                        .clone()
                        .wait_for(|released| *released)
                        .await;
                }
            })
        })
    };
    let fixture = create_fixture(FixtureOptions {
        calls: vec![("slow", "slow")],
        tools: vec![slow],
        mode: ToolExecutionMode::Sequential,
        on_tool_event: Some(on_tool_event),
        ..Default::default()
    })
    .await;
    let after_tool_called = Arc::new(AtomicBool::new(false));
    let recorder = Arc::clone(&after_tool_called);
    let after_tool: HookHandler = Arc::new(move |_invocation, _context| {
        let recorder = Arc::clone(&recorder);
        Box::pin(async move {
            recorder.store(true, Ordering::SeqCst);
            Ok(HookResult::AfterTool(None))
        })
    });
    fixture
        .lane
        .hooks()
        .on(
            crate::agent_core::harness::hooks::HookName::AfterTool,
            after_tool,
            None,
        )
        .unwrap();
    let running = spawn_drive_tools(&fixture);
    let _ = started_rx.clone().wait_for(|started| *started).await;
    let cancellation = tokio_util::sync::CancellationToken::new();
    fixture.drive.begin_abort(cancellation.clone());
    commit_cancel_requested(&fixture, 40).await;
    cancellation.cancel();
    fixture.drive.signal_abort();
    update_delivery_tx.send_replace(true);
    running.await.unwrap().unwrap();
    assert!(
        !after_tool_called.load(Ordering::SeqCst),
        "after_tool admission is refused after the abort"
    );
    let entry = fixture
        .session
        .get_entry(&fixture.result_entry_ids[0], background_context())
        .await
        .unwrap()
        .expect("slow result entry");
    match &entry {
        session::Entry::Message {
            message, terminate, ..
        } => {
            assert!(!matches!(terminate, Some(true)), "terminate is not durable");
            match message {
                AgentMessage::ToolResult(message) => assert!(message.is_error),
                _ => panic!("expected a toolResult message"),
            }
        }
        _ => panic!("expected a message entry"),
    }
}

// --- serialization-seam oracle comparison (node-captured) ---

const ORACLE_ABORTED: &str = r#"{"role":"toolResult","toolCallId":"call-0","toolName":"bash","content":[{"type":"text","text":"Tool execution was cancelled before completion."}],"isError":true,"timestamp":1234}"#;
const ORACLE_INTERRUPTED_NOCHECKPOINT: &str = r#"{"role":"toolResult","toolCallId":"call-0","toolName":"bash","content":[{"type":"text","text":"[Tool execution was interrupted. The preceding output is the latest durable progress snapshot; newer live output may be missing, and the external outcome is unknown.]"}],"isError":true,"timestamp":1234}"#;
const ORACLE_INTERRUPTED_CHECKPOINT: &str = r#"{"role":"toolResult","toolCallId":"call-0","toolName":"bash","content":[{"type":"text","text":"durable partial"},{"type":"text","text":"[Tool execution was interrupted. The preceding output is the latest durable progress snapshot; newer live output may be missing, and the external outcome is unknown.]"}],"details":{"progress":"kept"},"usage":{"input":1,"output":2,"cacheRead":0,"cacheWrite":0,"totalTokens":3,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"isError":true,"timestamp":1234}"#;
const ORACLE_TRUNCATED: &str = r#"{"role":"toolResult","toolCallId":"call-1","toolName":"quo\"ted","content":[{"type":"text","text":"Tool call \"quo\\\"ted\" was not executed because the assistant response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments."}],"isError":true,"timestamp":1234}"#;
const ORACLE_INTENT_EVENTS_RECOVERY: &str = r#"[{"type":"tool_start","lane":"main","runId":"op1","turnId":"turn-1","toolCallId":"call-0","toolName":"bash","args":{"value":"x"},"recovery":true}]"#;
const ORACLE_INTENT_EVENTS_PLAIN: &str = r#"[{"type":"tool_start","lane":"main","runId":"op1","turnId":"turn-1","toolCallId":"call-0","toolName":"bash","args":{"value":"x"}}]"#;
const ORACLE_OUTCOME_EVENTS_PLANNED: &str = r#"[{"type":"tool_start","lane":"main","runId":"op1","turnId":"turn-1","toolCallId":"call-0","toolName":"bash","args":{"value":"x"},"recovery":true},{"type":"tool_end","lane":"main","runId":"op1","turnId":"turn-1","toolCallId":"call-0","toolName":"bash","result":{"content":[{"type":"text","text":"done"}]},"isError":false,"terminate":false,"recovery":true}]"#;
const ORACLE_OUTCOME_EVENTS_PENDING: &str = r#"[{"type":"tool_end","lane":"main","runId":"op1","turnId":"turn-1","toolCallId":"call-0","toolName":"bash","result":{"content":[]},"isError":true,"terminate":true}]"#;

fn oracle_call(id: &str, name: &str) -> AgentToolCall {
    serde_json::from_value(serde_json::json!({"id": id, "name": name, "arguments": {}})).unwrap()
}

/// The oracle ran with `Date.now` stubbed to 1234; force the same timestamp
/// and serialize the message struct directly so the comparison is byte-level
/// (serde_json field order follows the declaration order, matching the
/// upstream object literal insertion order).
fn message_string(message: &ToolResultMessage) -> String {
    let mut message = message.clone();
    message.timestamp = 1234;
    serde_json::to_string(&AgentMessage::ToolResult(message)).unwrap()
}

fn assert_bytes_match_oracle(actual: String, oracle: &str) {
    assert_eq!(actual, oracle, "\nactual:   {actual}\noracle:   {oracle}");
}

#[test]
fn serialization_seams_match_the_node_oracle() {
    let call = oracle_call("call-0", "bash");
    let quoted_call = oracle_call("call-1", "quo\"ted");

    assert_bytes_match_oracle(
        message_string(&aborted_outcome(&call).message),
        ORACLE_ABORTED,
    );
    assert_bytes_match_oracle(
        message_string(&interrupted_outcome(&call, None).message),
        ORACLE_INTERRUPTED_NOCHECKPOINT,
    );
    let checkpoint = AgentToolResult {
        content: vec![text_block("durable partial")],
        details: Some(serde_json::json!({"progress": "kept"})),
        usage: Some(
            serde_json::from_value(serde_json::json!({
                "input": 1, "output": 2, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 3,
                "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0},
            }))
            .unwrap(),
        ),
        terminate: None,
    };
    assert_bytes_match_oracle(
        message_string(&interrupted_outcome(&call, Some(checkpoint)).message),
        ORACLE_INTERRUPTED_CHECKPOINT,
    );
    assert_bytes_match_oracle(
        message_string(&truncated_outcome(&quoted_call).message),
        ORACLE_TRUNCATED,
    );

    let intent_events = |recovery: bool| {
        vec![ToolEvent::ToolStart {
            lane: "main".to_owned(),
            run_id: "op1".to_owned(),
            turn_id: "turn-1".to_owned(),
            tool_call_id: "call-0".to_owned(),
            tool_name: "bash".to_owned(),
            args: serde_json::json!({"value": "x"}),
            recovery,
        }]
    };
    assert_bytes_match_oracle(
        serde_json::to_string(&intent_events(true)).unwrap(),
        ORACLE_INTENT_EVENTS_RECOVERY,
    );
    assert_bytes_match_oracle(
        serde_json::to_string(&intent_events(false)).unwrap(),
        ORACLE_INTENT_EVENTS_PLAIN,
    );
    assert_bytes_match_oracle(
        serde_json::to_string(&vec![
            ToolEvent::ToolStart {
                lane: "main".to_owned(),
                run_id: "op1".to_owned(),
                turn_id: "turn-1".to_owned(),
                tool_call_id: "call-0".to_owned(),
                tool_name: "bash".to_owned(),
                args: serde_json::json!({"value": "x"}),
                recovery: true,
            },
            ToolEvent::ToolEnd {
                lane: "main".to_owned(),
                run_id: "op1".to_owned(),
                turn_id: "turn-1".to_owned(),
                tool_call_id: "call-0".to_owned(),
                tool_name: "bash".to_owned(),
                result: AgentToolResult {
                    content: vec![text_block("done")],
                    details: None,
                    usage: None,
                    terminate: None,
                },
                is_error: false,
                terminate: false,
                recovery: true,
            },
        ])
        .unwrap(),
        ORACLE_OUTCOME_EVENTS_PLANNED,
    );
    assert_bytes_match_oracle(
        serde_json::to_string(&vec![ToolEvent::ToolEnd {
            lane: "main".to_owned(),
            run_id: "op1".to_owned(),
            turn_id: "turn-1".to_owned(),
            tool_call_id: "call-0".to_owned(),
            tool_name: "bash".to_owned(),
            result: AgentToolResult {
                content: Vec::new(),
                details: None,
                usage: None,
                terminate: None,
            },
            is_error: true,
            terminate: true,
            recovery: false,
        }])
        .unwrap(),
        ORACLE_OUTCOME_EVENTS_PENDING,
    );
}
