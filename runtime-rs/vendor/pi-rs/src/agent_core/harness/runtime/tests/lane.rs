//! Port of upstream `packages/agent/test/harness/runtime/lane.test.ts`
//! ("runtime Lane commands"): the 12 acceptance cases for the serialized
//! command line. JS `beforeNextCommit` gates become an armed one-shot
//! `ControlledMemoryStorage` commit gate with started/release channels.

use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use tokio::sync::oneshot;

use super::super::durable::{LaneState, Operation, OperationPhase, OperationScope, OperationState};
use super::super::lane::{
    ContinueOperationResult, Lane, LaneCommand, LanePatch, LaneSealed, OperationCommand,
    OperationRequest, SealKind,
};
use crate::agent_core::harness::context::Context;
use crate::agent_core::harness::runtime::lane::{EmitBatch, RuntimeConfig};
use crate::agent_core::harness::runtime::restore::restore_lane;
use crate::agent_core::harness::session::{
    self as session, CommitResult, Control, InboxItem, InboxItemKind, LaneConfiguration, LaneModel,
    MemoryStorage, Session as _, SessionMetadata, Storage, StorageBackedSession, StoredValue,
    ValueAddress, Write,
};
use crate::agent_core::harness::{background_context, DEFAULT_COMPACTION_SETTINGS};
use crate::agent_core::types::{QueueMode, ThinkingLevel, ToolExecutionMode};
use crate::ai::models::faux::{faux_provider, FauxProviderOptions};
use crate::ai::models::{create_models, CreateModelsOptions};

fn background() -> Context {
    background_context()
}

// ---------------------------------------------------------------------------
// ControlledMemoryStorage: commit gate + failure injection
// ---------------------------------------------------------------------------

#[derive(Default)]
struct GateInner {
    armed: std::sync::atomic::AtomicBool,
    started: Mutex<Option<oneshot::Sender<()>>>,
    release: Mutex<Option<oneshot::Receiver<()>>>,
    fail_with: Mutex<Option<String>>,
}

#[derive(Clone)]
struct ControlledGate {
    inner: Arc<GateInner>,
}

impl ControlledGate {
    fn new() -> Self {
        Self {
            inner: Arc::new(GateInner::default()),
        }
    }

    /// Arm the gate: the next commit signals `started` and blocks until the
    /// test sends on `release` (or fails when armed to fail).
    fn arm(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        self.inner
            .armed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let (started_tx, started_rx) = oneshot::channel();
        *self.inner.started.lock().unwrap() = Some(started_tx);
        let (release_tx, release_rx) = oneshot::channel();
        *self.inner.release.lock().unwrap() = Some(release_rx);
        (started_rx, release_tx)
    }

    fn fail_with(&self, message: &str) {
        self.inner
            .armed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        *self.inner.fail_with.lock().unwrap() = Some(message.to_string());
    }
}

struct ControlledMemoryStorage {
    base: Arc<MemoryStorage>,
    gate: ControlledGate,
}

impl ControlledMemoryStorage {
    fn new(gate: ControlledGate) -> Arc<Self> {
        Arc::new(Self {
            base: Arc::new(MemoryStorage::new(session::MemoryStorageOptions::default())),
            gate,
        })
    }
}

impl Storage for ControlledMemoryStorage {
    fn get_entries<'a>(
        &'a self,
        ids: &[String],
        context: Context,
    ) -> futures::future::BoxFuture<
        'a,
        anyhow::Result<std::collections::HashMap<String, session::Entry>>,
    > {
        self.base.get_entries(ids, context)
    }

    fn get_value<'a>(
        &'a self,
        address: &ValueAddress,
        context: Context,
    ) -> futures::future::BoxFuture<'a, anyhow::Result<Option<StoredValue>>> {
        self.base.get_value(address, context)
    }

    fn scan_values<'a>(
        &'a self,
        prefix: &ValueAddress,
        context: Context,
    ) -> futures::future::BoxFuture<'a, anyhow::Result<Vec<StoredValue>>> {
        self.base.scan_values(prefix, context)
    }

    fn read_list<'a>(
        &'a self,
        address: &ValueAddress,
        options: Option<&session::ListReadOptions>,
        context: Context,
    ) -> futures::future::BoxFuture<'a, anyhow::Result<Vec<session::ListElement>>> {
        self.base.read_list(address, options, context)
    }

    fn scan_branch<'a>(
        &'a self,
        query: &session::StorageBranchScan,
        context: Context,
    ) -> futures::future::BoxFuture<'a, anyhow::Result<Vec<session::Entry>>> {
        self.base.scan_branch(query, context)
    }

    fn scan_branch_structure<'a>(
        &'a self,
        query: &session::StorageBranchScan,
        context: Context,
    ) -> futures::future::BoxFuture<'a, anyhow::Result<Vec<session::EntryStructure>>> {
        self.base.scan_branch_structure(query, context)
    }

    fn scan_entries<'a>(
        &'a self,
        query: &session::EntryScan,
        context: Context,
    ) -> futures::future::BoxFuture<'a, anyhow::Result<Vec<session::Entry>>> {
        self.base.scan_entries(query, context)
    }

    fn scan_usage<'a>(
        &'a self,
        query: &session::UsageScan,
        context: Context,
    ) -> futures::future::BoxFuture<'a, anyhow::Result<Vec<session::UsageRow>>> {
        self.base.scan_usage(query, context)
    }

    fn get_stats<'a>(
        &'a self,
        context: Context,
    ) -> futures::future::BoxFuture<'a, anyhow::Result<session::SessionStats>> {
        self.base.get_stats(context)
    }

    fn close<'a>(&'a self, context: Context) -> futures::future::BoxFuture<'a, anyhow::Result<()>> {
        self.base.close(context)
    }

    fn commit<'a>(
        &'a self,
        writes: Vec<Write>,
        context: Context,
    ) -> futures::future::BoxFuture<'a, anyhow::Result<CommitResult>> {
        Box::pin(async move {
            if self
                .gate
                .inner
                .armed
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                let started_tx = self.gate.inner.started.lock().unwrap().take();
                if let Some(tx) = started_tx {
                    let _ = tx.send(());
                }
                let failure = self.gate.inner.fail_with.lock().unwrap().take();
                if let Some(message) = failure {
                    return Err(anyhow!(message));
                }
                let release_rx = self.gate.inner.release.lock().unwrap().take();
                if let Some(rx) = release_rx {
                    let _ = rx.await;
                }
            }
            self.base.commit(writes, context).await
        })
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct TestLane {
    lane: Arc<Lane>,
    gate: ControlledGate,
    session: Arc<StorageBackedSession>,
}

fn identity_emit_batch() -> EmitBatch {
    Arc::new(|_events, _context| Box::pin(async { Ok(()) }))
}

async fn create_lane() -> TestLane {
    create_lane_with_emit(None).await
}

async fn create_lane_with_emit(emit_batch: Option<EmitBatch>) -> TestLane {
    eprintln!("[lane-test] create_lane start");
    let gate = ControlledGate::new();
    let storage = ControlledMemoryStorage::new(gate.clone());
    let session = Arc::new(StorageBackedSession::new(
        SessionMetadata {
            id: "runtime-lane-test".into(),
            created_at: 1,
            storage_version: 1,
            ..Default::default()
        },
        storage.clone(),
    ));

    let configuration = LaneConfiguration {
        model: LaneModel {
            provider: "faux".to_string(),
            model_id: "faux-1".to_string(),
        },
        thinking_level: ThinkingLevel::Off,
        active_tool_names: Vec::new(),
    };
    let writes: Vec<Write> = vec![
        session::set_value(&session::branch_tip("main"), serde_json::Value::Null),
        session::set_value(
            &session::lane_config("main"),
            session::lane_configuration_value(&configuration),
        ),
        session::set_value(
            &session::lane_state("main"),
            serde_json::json!({"currentOperationId":null,"lastOperationId":null,"inbox":[]}),
        ),
    ];
    session
        .mutate(
            move |reader, context| {
                Box::pin(async move { reader.commit(writes, context).await.map(|_| ()) })
            },
            background(),
        )
        .await
        .unwrap();

    eprintln!("[lane-test] session seeded");
    let faux = faux_provider(FauxProviderOptions::default());
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));

    eprintln!("[lane-test] models built");
    let state = restore_lane(session.as_ref(), "main", background())
        .await
        .expect("lane restores");
    eprintln!("[lane-test] lane restored");

    let lane = Lane::new(
        "main",
        session.clone(),
        models,
        crate::agent_core::harness::hooks::HookRegistry::new(Arc::new(
            |_error: anyhow::Error,
             _hook: crate::agent_core::harness::hooks::HookName,
             _message: String,
             _context: Context| { Box::pin(async {}) },
        )),
        state,
        Arc::new(|error: anyhow::Error| error),
        emit_batch.unwrap_or_else(identity_emit_batch),
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
            tool_execution: ToolExecutionMode::Parallel,
            entry_projectors: None,
        }),
    );

    TestLane {
        lane,
        gate,
        session,
    }
}

/// The upstream `setThinkingLevel` helper: a direct lane command committing a
/// configuration update, optionally recording the observed pre-commit level.
async fn set_thinking_level(
    lane: &Arc<Lane>,
    level: ThinkingLevel,
    observed: Option<Arc<Mutex<Vec<String>>>>,
) -> anyhow::Result<()> {
    lane.command(
        move |state: &LaneState, _reader| {
            let observed = observed.clone();
            Box::pin(async move {
                if let Some(observed) = observed {
                    observed
                        .lock()
                        .unwrap()
                        .push(format!("{:?}", state.configuration.thinking_level).to_lowercase());
                }
                let mut next = state.clone();
                next.configuration.thinking_level = level;
                Ok::<LaneCommand<()>, anyhow::Error>(LaneCommand::Commit {
                    writes: vec![session::set_value(
                        &session::lane_config("main"),
                        session::lane_configuration_value(&next.configuration),
                    )],
                    next,
                    materialize: Box::new(|_commit: &CommitResult| {}),
                    events: None,
                })
            })
        },
        background(),
    )
    .await
}

async fn stored_lane_config(session: &StorageBackedSession) -> LaneConfiguration {
    let stored: StoredValue = session
        .mutate(
            |reader, context| {
                let address = session::lane_config("main");
                Box::pin(async move { reader.get_value(&address, context).await })
            },
            background(),
        )
        .await
        .unwrap()
        .expect("lane config stored");
    serde_json::from_value(stored.value).expect("stored configuration deserializes")
}

fn faux_model() -> LaneModel {
    LaneModel {
        provider: "faux".to_string(),
        model_id: "faux-1".to_string(),
    }
}

// ---------------------------------------------------------------------------
// The 12 upstream acceptance cases
// ---------------------------------------------------------------------------

/// 1. reads and replaces configuration from owned state.
#[tokio::test]
async fn reads_and_replaces_configuration_from_owned_state() {
    let harness = create_lane().await;
    let lane = harness.lane;
    let active_tool_names = vec!["read".to_string()];

    lane.set_model(faux_model(), background()).await.unwrap();
    lane.set_thinking_level(ThinkingLevel::High, background())
        .await
        .unwrap();
    lane.set_active_tools(active_tool_names.clone(), background())
        .await
        .unwrap();

    let model = lane
        .get_model(background())
        .await
        .unwrap()
        .expect("model resolves");
    assert_eq!(model.id, "faux-1");
    assert_eq!(
        lane.get_thinking_level(background()).await.unwrap(),
        ThinkingLevel::High
    );
    assert_eq!(
        lane.get_active_tools(background()).await.unwrap(),
        active_tool_names
    );

    let stored = stored_lane_config(&harness.session).await;
    assert_eq!(stored.model, faux_model());
    assert_eq!(stored.thinking_level, ThinkingLevel::High);
    assert_eq!(stored.active_tool_names, vec!["read".to_string()]);
}

/// 2. derives queued configuration updates from the latest committed state.
#[tokio::test]
async fn derives_queued_configuration_updates_from_latest_committed_state() {
    let harness = create_lane().await;
    let lane = Arc::clone(&harness.lane);

    let (started, release) = harness.gate.arm();
    let model_update = tokio::spawn(async move {
        lane.set_model(faux_model(), background()).await.unwrap();
    });
    let _ = started.await;

    let thinking_lane = Arc::clone(&harness.lane);
    let thinking_update = tokio::spawn(async move {
        thinking_lane
            .set_thinking_level(ThinkingLevel::High, background())
            .await
    });

    // Release the model commit; the queued thinking update then derives from
    // the latest committed state (the model update), not a stale snapshot.
    release.send(()).unwrap();
    model_update.await.unwrap();
    thinking_update.await.unwrap().unwrap();

    let configuration = harness.lane.state().configuration;
    assert_eq!(configuration.model, faux_model());
    assert_eq!(configuration.thinking_level, ThinkingLevel::High);
    assert!(configuration.active_tool_names.is_empty());
}

/// 3. returns a promise value without holding the lane line.
#[tokio::test]
async fn returns_a_value_without_holding_the_lane_line() {
    let harness = create_lane().await;
    let lane = harness.lane;

    let (completion_tx, completion_rx) = oneshot::channel::<()>();
    let completion_rx = Arc::new(Mutex::new(Some(completion_rx)));
    let result = lane
        .command(
            move |_state: &LaneState, _reader| {
                let completion_rx = Arc::clone(&completion_rx);
                Box::pin(async move {
                    let rx = completion_rx
                        .lock()
                        .unwrap()
                        .take()
                        .expect("planner runs once");
                    Ok::<LaneCommand<oneshot::Receiver<()>>, anyhow::Error>(LaneCommand::Return {
                        result: rx,
                    })
                })
            },
            background(),
        )
        .await
        .expect("command resolves with the pending value");

    // The line is free while the value is still pending.
    lane.set_thinking_level(ThinkingLevel::High, background())
        .await
        .unwrap();

    completion_tx.send(()).unwrap();
    result.await.unwrap();
}

/// 4. returns an expected rejection without faulting the lane.
#[tokio::test]
async fn returns_an_expected_rejection_without_faulting() {
    let harness = create_lane().await;
    let lane = harness.lane;

    let error = lane
        .command(
            move |_state: &LaneState, _reader| {
                Box::pin(async move {
                    Ok::<LaneCommand<()>, anyhow::Error>(LaneCommand::Reject {
                        error: anyhow!("declined"),
                    })
                })
            },
            background(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "declined");

    // The lane is still open and fully usable.
    assert!(lane.state().tip_id.is_none());
    lane.set_thinking_level(ThinkingLevel::High, background())
        .await
        .unwrap();
}

/// 5. passes bounded reads and commit metadata through the serialized command.
#[tokio::test]
async fn passes_bounded_reads_and_commit_metadata_through_the_serialized_command() {
    let harness = create_lane().await;
    let lane = Arc::clone(&harness.lane);

    let stored_configuration: Arc<Mutex<Option<LaneConfiguration>>> = Arc::default();
    let memory_published: Arc<Mutex<bool>> = Arc::default();

    let read_capture = Arc::clone(&stored_configuration);
    let published_flag = Arc::clone(&memory_published);
    let lane_for_materialize = Arc::clone(&lane);

    let commit: CommitResult = lane
        .command(
            move |state: &LaneState, reader| {
                let read_capture = Arc::clone(&read_capture);
                let published_flag = Arc::clone(&published_flag);
                let lane_for_materialize = Arc::clone(&lane_for_materialize);
                Box::pin(async move {
                    let stored = reader
                        .get_value(&session::lane_config("main"), background())
                        .await?
                        .expect("lane config present");
                    *read_capture.lock().unwrap() =
                        Some(serde_json::from_value::<LaneConfiguration>(stored.value).unwrap());

                    let mut next = state.clone();
                    next.configuration.thinking_level = ThinkingLevel::High;

                    Ok::<LaneCommand<CommitResult>, anyhow::Error>(LaneCommand::Commit {
                        writes: vec![session::set_value(
                            &session::lane_config("main"),
                            session::lane_configuration_value(&next.configuration),
                        )],
                        next,
                        materialize: Box::new(move |commit: &CommitResult| {
                            // Owned state is already the committed projection
                            // when materialize runs synchronously.
                            *published_flag.lock().unwrap() =
                                lane_for_materialize.state().configuration.thinking_level
                                    == ThinkingLevel::High;
                            commit.clone()
                        }),
                        events: None,
                    })
                })
            },
            background(),
        )
        .await
        .unwrap();

    assert_eq!(
        stored_configuration.lock().unwrap().clone().unwrap(),
        LaneConfiguration {
            model: faux_model(),
            thinking_level: ThinkingLevel::Off,
            active_tool_names: Vec::new(),
        }
    );
    assert!(*memory_published.lock().unwrap());
    assert_eq!(commit.seqs.len(), 1);
}

/// 6. materialization is synchronous at the type level (upstream rejects
/// thenable materializers; the Rust port enforces it statically and the
/// synchronous order is pinned here).
#[tokio::test]
async fn materialization_is_synchronous_at_the_type_level() {
    let harness = create_lane().await;
    let order: Arc<Mutex<Vec<&'static str>>> = Arc::default();
    let order_sink = Arc::clone(&order);

    harness
        .lane
        .command(
            move |state: &LaneState, _reader| {
                let order_sink = Arc::clone(&order_sink);
                Box::pin(async move {
                    Ok::<LaneCommand<()>, anyhow::Error>(LaneCommand::Commit {
                        writes: vec![],
                        next: state.clone(),
                        materialize: Box::new(move |_commit: &CommitResult| {
                            order_sink.lock().unwrap().push("materialize");
                        }),
                        events: None,
                    })
                })
            },
            background(),
        )
        .await
        .unwrap();
    // Materialize ran synchronously before the command resolved.
    assert_eq!(order.lock().unwrap().as_slice(), ["materialize"]);
}

/// 7. preserves committed memory when synchronous event publication fails.
#[tokio::test]
async fn preserves_committed_memory_when_event_publication_fails() {
    let failing_emit: EmitBatch = Arc::new(|_events, _context| {
        Box::pin(async { Err(anyhow!("DataCloneError: event could not be cloned.")) })
    });
    let harness = create_lane_with_emit(Some(failing_emit)).await;
    let lane = harness.lane;

    let error = lane
        .set_thinking_level(ThinkingLevel::High, background())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("DataCloneError"));

    // Committed memory stays published despite the delivery failure.
    assert_eq!(
        lane.state().configuration.thinking_level,
        ThinkingLevel::High
    );
    let stored = stored_lane_config(&harness.session).await;
    assert_eq!(stored.thinking_level, ThinkingLevel::High);
}

/// 8. rejects work after sealing while an admitted commit finishes.
#[tokio::test]
async fn rejects_work_after_sealing_while_an_admitted_commit_finishes() {
    let harness = create_lane().await;
    let lane = Arc::clone(&harness.lane);

    let (started, release) = harness.gate.arm();
    let admitted_lane = Arc::clone(&lane);
    let admitted = tokio::spawn(async move {
        admitted_lane
            .set_thinking_level(ThinkingLevel::High, background())
            .await
    });
    let _ = started.await;

    harness.lane.seal(SealKind::Closed, "lane closed");

    // New work is rejected with the seal error.
    let tip_error = harness
        .lane
        .command(
            move |_state: &LaneState, _reader| {
                Box::pin(async move {
                    Ok::<LaneCommand<()>, anyhow::Error>(LaneCommand::Return { result: () })
                })
            },
            background(),
        )
        .await
        .unwrap_err();
    // Upstream asserts the rejected error IS the sealed error object; the port
    // shares the same Arc, so the message (and kind) identify it.
    assert!(tip_error
        .downcast_ref::<Arc<LaneSealed>>()
        .is_some_and(|sealed| sealed.kind == SealKind::Closed && sealed.message == "lane closed"));
    let low_error = harness
        .lane
        .set_thinking_level(ThinkingLevel::Low, background())
        .await
        .unwrap_err();
    assert!(low_error
        .downcast_ref::<Arc<LaneSealed>>()
        .is_some_and(|sealed| sealed.kind == SealKind::Closed && sealed.message == "lane closed"));

    release.send(()).unwrap();
    admitted.await.unwrap().unwrap();

    // The admitted commit finished and published.
    assert_eq!(
        harness.lane.state().configuration.thinking_level,
        ThinkingLevel::High
    );
}

/// 9. publishes memory only after the durable commit succeeds.
#[tokio::test]
async fn publishes_memory_only_after_the_durable_commit_succeeds() {
    let harness = create_lane().await;
    let (started, release) = harness.gate.arm();

    let command_lane = Arc::clone(&harness.lane);
    let command = tokio::spawn(async move {
        command_lane
            .set_thinking_level(ThinkingLevel::High, background())
            .await
    });
    let _ = started.await;

    // While the durable commit is in flight, owned memory still shows "off".
    assert_eq!(
        harness.lane.state().configuration.thinking_level,
        ThinkingLevel::Off
    );

    release.send(()).unwrap();
    command.await.unwrap().unwrap();
    assert_eq!(
        harness.lane.state().configuration.thinking_level,
        ThinkingLevel::High
    );
    let stored = stored_lane_config(&harness.session).await;
    assert_eq!(stored.thinking_level, ThinkingLevel::High);
}

/// 10. preserves memory when the durable commit fails.
#[tokio::test]
async fn preserves_memory_when_the_durable_commit_fails() {
    let harness = create_lane().await;
    harness.gate.fail_with("commit failed");

    let error = harness
        .lane
        .set_thinking_level(ThinkingLevel::High, background())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "commit failed");

    assert_eq!(
        harness.lane.state().configuration.thinking_level,
        ThinkingLevel::Off
    );
    let stored = stored_lane_config(&harness.session).await;
    assert_eq!(stored.thinking_level, ThinkingLevel::Off);
}

/// 11. diverts ordinary work but settles against the latest cancelled control.
#[tokio::test]
async fn diverts_ordinary_work_but_settles_against_latest_cancelled_control() {
    let harness = create_lane().await;
    let lane = harness.lane;

    let admission = lane
        .accept(
            &OperationRequest::Prompt {
                prompt: "hello".to_string(),
            },
            background(),
        )
        .await
        .unwrap()
        .expect("admission ok");
    let operation = lane.state().operation.expect("accepted operation");
    assert_eq!(operation.meta.operation_id, admission.operation_id);

    // Force the operation into cancel_requested via a direct command (upstream
    // writes the cancelled StartingOperation state directly too).
    let cancelled = OperationState {
        scope: OperationScope {
            control: Control::CancelRequested { requested_at: 1 },
            ..operation.state.scope.clone()
        },
        phase: OperationPhase::Starting,
    };
    let operation_id = Arc::new(operation.meta.operation_id.clone());
    let operation_meta = Arc::new(operation.meta.clone());
    let cancelled = Arc::new(cancelled);
    lane.command(
        move |state: &LaneState, _reader| {
            let operation_meta = Arc::clone(&operation_meta);
            let cancelled = Arc::clone(&cancelled);
            let operation_id = Arc::clone(&operation_id);
            Box::pin(async move {
                let mut next = state.clone();
                next.operation = Some(Operation {
                    meta: (*operation_meta).clone(),
                    state: (*cancelled).clone(),
                });
                Ok::<LaneCommand<()>, anyhow::Error>(LaneCommand::Commit {
                    writes: vec![session::set_value(
                        &session::operation_state(&operation_id),
                        serde_json::to_value(&*cancelled)?,
                    )],
                    next,
                    materialize: Box::new(|_commit: &CommitResult| {}),
                    events: None,
                })
            })
        },
        background(),
    )
    .await
    .unwrap();

    // continueOperation sees the cancelled control and never invokes the
    // planner.
    let continued = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let continued_sink = Arc::clone(&continued);
    let diverted = lane
        .continue_operation(
            move |_state: &LaneState, _current: &OperationState, _meta, _reader| {
                continued_sink.store(true, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async move {
                    Ok::<OperationCommand<()>, anyhow::Error>(OperationCommand::Return {
                        result: (),
                    })
                })
            },
            background(),
        )
        .await
        .unwrap();
    assert_eq!(diverted, ContinueOperationResult::CancelRequested);
    assert!(!continued.load(std::sync::atomic::Ordering::SeqCst));

    // settleOperation runs against the latest cancelled control and can still
    // commit lane patches.
    let settled = lane
        .settle_operation(
            move |_state: &LaneState, current: &OperationState, _meta, _reader| {
                Box::pin(async move {
                    assert!(matches!(
                        current.scope.control,
                        Control::CancelRequested { .. }
                    ));
                    let inbox = vec![InboxItem {
                        entry_id: "accepted-during-cancellation".to_string(),
                        kind: InboxItemKind::Write,
                    }];
                    Ok::<OperationCommand<Vec<InboxItem>>, anyhow::Error>(
                        OperationCommand::Commit {
                            writes: Vec::new(),
                            operation_state: current.clone(),
                            lane: Some(LanePatch {
                                inbox: Some(inbox.clone()),
                                ..Default::default()
                            }),
                            materialize: Box::new(move |_commit: &CommitResult| inbox.clone()),
                            events: None,
                        },
                    )
                })
            },
            background(),
        )
        .await
        .unwrap();

    assert_eq!(
        settled,
        vec![InboxItem {
            entry_id: "accepted-during-cancellation".to_string(),
            kind: InboxItemKind::Write,
        }]
    );
    assert_eq!(lane.state().inbox, settled);

    let stored: StoredValue = harness
        .session
        .mutate(
            |reader, context| {
                let address = session::lane_state("main");
                Box::pin(async move { reader.get_value(&address, context).await })
            },
            background(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.value["inbox"],
        serde_json::json!([
            {"entryId": "accepted-during-cancellation", "kind": "write"}
        ])
    );
}

/// 12. plans queued commands from the latest committed memory.
#[tokio::test]
async fn plans_queued_commands_from_the_latest_committed_memory() {
    let harness = create_lane().await;
    let observed: Arc<Mutex<Vec<String>>> = Arc::default();

    let (started, release) = harness.gate.arm();

    let observed_sink = Arc::clone(&observed);
    let first_lane = Arc::clone(&harness.lane);
    let first = tokio::spawn(async move {
        set_thinking_level(&first_lane, ThinkingLevel::High, Some(observed_sink)).await
    });

    // The first command is queued on the line; its planner saw "off".
    let _ = started.await;
    assert_eq!(*observed.lock().unwrap(), vec!["off".to_string()]);

    // The second command also records the level its planner observed.
    let observed_sink = Arc::clone(&observed);
    let second_lane = Arc::clone(&harness.lane);
    let second = tokio::spawn(async move {
        set_thinking_level(&second_lane, ThinkingLevel::Medium, Some(observed_sink)).await
    });

    release.send(()).unwrap();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(
        *observed.lock().unwrap(),
        vec!["off".to_string(), "high".to_string()]
    );
    assert_eq!(
        harness.lane.state().configuration.thinking_level,
        ThinkingLevel::Medium
    );
}
