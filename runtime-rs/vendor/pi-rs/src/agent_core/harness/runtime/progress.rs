//! Durable stream progress from upstream runtime/progress.ts. Writes enter an
//! ordered lane-command chain; sealing stops admission, and drain observes
//! the latest accepted write (including its error).
use crate::agent_core::harness::context::Context;
use crate::agent_core::harness::runtime::drive_pass::Drive;
use crate::agent_core::harness::runtime::durable::LaneState;
use crate::agent_core::harness::runtime::lane::{Lane, LaneCommand};
use crate::agent_core::harness::session::types::Write;
use crate::agent_core::harness::session::{
    append_list, pending_assistant_frames, pending_tool_output, set_value, AscDescOrder,
    ListCursor, ListReadOptions, SessionMutationReader,
};
use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use serde_json::Value;
use std::sync::Arc;

/// Frames retain their exact durable JSON, including unknown fields and null.
/// They are not coerced into a different provider stream-event representation.
pub async fn read_assistant_frames(
    reader: &dyn SessionMutationReader,
    operation_id: &str,
    response_entry_id: &str,
    context: Context,
) -> anyhow::Result<Vec<Value>> {
    let address = pending_assistant_frames(operation_id, response_entry_id);
    let mut frames = Vec::new();
    let mut cursor = None;
    loop {
        let options = ListReadOptions {
            cursor,
            order: Some(AscDescOrder::Asc),
            limit: Some(1_000),
        };
        let page = reader
            .read_list(&address, Some(&options), context.clone())
            .await?;
        let done = page.len() < 1_000;
        cursor = page.last().map(|item| ListCursor { seq: item.seq });
        frames.extend(page.into_iter().map(|item| item.value));
        if done {
            return Ok(frames);
        }
    }
}

// --- progress channels (progress.ts:10-14, 37-88, 90-117) ---

/// Upstream `ProgressChannel` (`progress.ts:10-14`): a fire-and-forget write
/// channel whose `drain` awaits only the latest accepted write; writes after
/// `seal` are ignored.
pub struct ProgressChannel<T: Send + Sync + Clone + 'static> {
    lane: std::sync::Arc<Lane>,
    context: Context,
    commit_write: Arc<dyn Fn(&T) -> Write + Send + Sync>,
    still_owns: Arc<dyn Fn(&LaneState) -> bool + Send + Sync>,
    sealed: std::sync::atomic::AtomicBool,
    latest: std::sync::Mutex<Option<ProgressWrite>>,
}

type ProgressWrite = Shared<BoxFuture<'static, Result<(), Arc<anyhow::Error>>>>;

/// Retain the underlying error as a source when replaying a shared write result.
#[derive(Debug)]
struct ProgressWriteError(Arc<anyhow::Error>);
impl std::fmt::Display for ProgressWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for ProgressWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref().as_ref())
    }
}

impl<T: Send + Sync + Clone + 'static> ProgressChannel<T> {
    /// Upstream `write` (`progress.ts:46-59`): commit one staged item while
    /// the operation still owns the channel. The upstream command queue is
    /// entered synchronously; chain tasks here so spawn scheduling cannot
    /// reorder writes. Earlier failures do not poison later commands.
    pub fn write(&self, item: T) {
        use std::sync::atomic::Ordering;
        let mut latest = self
            .latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.sealed.load(Ordering::SeqCst) {
            return;
        }
        let lane = self.lane.clone();
        let context = self.context.clone();
        let commit_write = Arc::clone(&self.commit_write);
        let still_owns = Arc::clone(&self.still_owns);
        let previous = latest.clone();
        let task = tokio::spawn(async move {
            if let Some(previous) = previous {
                let _ = previous.await;
            }
            lane.command(
                move |projection, _reader| {
                    let commit_write = Arc::clone(&commit_write);
                    let still_owns = Arc::clone(&still_owns);
                    let item = item.clone();
                    Box::pin(async move {
                        if !still_owns(projection) {
                            return Ok(LaneCommand::Return { result: () });
                        }
                        Ok(LaneCommand::Commit {
                            writes: vec![commit_write(&item)],
                            next: projection.clone(),
                            materialize: Box::new(|_commit| {}),
                            events: None,
                        })
                    })
                },
                context,
            )
            .await
        });
        *latest = Some(
            async move {
                task.await
                    .map_err(anyhow::Error::new)
                    .and_then(|result| result)
                    .map_err(Arc::new)
            }
            .boxed()
            .shared(),
        );
    }

    /// Upstream `seal` (`progress.ts:60-62`).
    pub fn seal(&self) {
        use std::sync::atomic::Ordering;
        let _admission = self
            .latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.sealed.store(true, Ordering::SeqCst);
    }

    /// Upstream `drain` (`progress.ts:63-65`): wait for the newest write.
    pub async fn drain(&self) -> anyhow::Result<()> {
        let latest = self
            .latest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(task) = latest {
            task.await
                .map_err(|error| anyhow::Error::new(ProgressWriteError(error)))?;
        }
        Ok(())
    }
}

/// Upstream `openProgress` (`progress.ts:37-44`).
fn open_progress<T: Send + Sync + Clone + 'static>(
    lane: std::sync::Arc<Lane>,
    context: Context,
    commit_write: Arc<dyn Fn(&T) -> Write + Send + Sync>,
    still_owns: Arc<dyn Fn(&LaneState) -> bool + Send + Sync>,
) -> ProgressChannel<T> {
    ProgressChannel {
        lane,
        context,
        commit_write,
        still_owns,
        sealed: std::sync::atomic::AtomicBool::new(false),
        latest: std::sync::Mutex::new(None),
    }
}

/// Upstream `openFrameProgress` (`progress.ts:69-88`): staged assistant
/// message frames for one response entry, owned while the run sits in an
/// assistant/deferred effect-pending phase for the same entry.
pub fn open_frame_progress(
    lane: &std::sync::Arc<Lane>,
    drive: &Drive,
    response_entry_id: &str,
) -> ProgressChannel<crate::ai::frame::AssistantMessageFrame> {
    let response_entry_id = response_entry_id.to_string();
    let address = pending_assistant_frames(drive.operation_id(), &response_entry_id);
    let still_owns: Arc<dyn Fn(&LaneState) -> bool + Send + Sync> = Arc::new(move |state| {
        let Some(operation) = &state.operation else {
            return false;
        };
        match &operation.state.phase {
            crate::agent_core::harness::runtime::durable::OperationPhase::AssistantEffectPending {
                response_entry_id: owned,
                ..
            }
            | crate::agent_core::harness::runtime::durable::OperationPhase::DeferredEffectPending {
                response_entry_id: owned,
                ..
            } => owned == &response_entry_id,
            _ => false,
        }
    });
    open_progress(
        Arc::clone(lane),
        drive.context().clone(),
        Arc::new(move |frame| {
            // Self-produced frames are JSON-serializable by construction.
            append_list(
                &address,
                serde_json::to_value(frame).expect("frame serializes"),
            )
        }),
        still_owns,
    )
}

/// Upstream `openToolProgress` (`progress.ts:90-117`): staged tool output for
/// one invocation, owned while the matching tool call is effect-pending.
pub fn open_tool_progress(
    lane: &std::sync::Arc<Lane>,
    drive: &Drive,
    turn_id: &str,
    source_index: usize,
    invocation_id: &str,
) -> ProgressChannel<serde_json::Value> {
    let address = pending_tool_output(drive.operation_id(), invocation_id);
    let turn_id = turn_id.to_string();
    let invocation_id = invocation_id.to_string();
    let still_owns: Arc<dyn Fn(&LaneState) -> bool + Send + Sync> = Arc::new(move |state| {
        let Some(operation) = &state.operation else {
            return false;
        };
        let crate::agent_core::harness::runtime::durable::OperationPhase::Tools { batch } =
            &operation.state.phase
        else {
            return false;
        };
        batch.turn_id == turn_id
            && batch.calls.iter().any(|call| {
                call.source_index == source_index
                    && call.result_entry_id == invocation_id
                    && matches!(
                        call.state,
                        crate::agent_core::harness::runtime::durable::ToolCallState::EffectPending { .. }
                    )
            })
    });
    open_progress(
        Arc::clone(lane),
        drive.context().clone(),
        Arc::new(move |snapshot| set_value(&address, snapshot.clone())),
        still_owns,
    )
}

#[cfg(test)]
mod progress_channel_tests {
    use super::*;
    use crate::agent_core::harness::runtime::lane::{EmitBatch, RuntimeConfig};
    use crate::agent_core::harness::runtime::restore::restore_lane;
    use crate::agent_core::harness::session;
    use crate::agent_core::harness::session::Session as _;
    use crate::agent_core::harness::session::{
        LaneConfiguration, LaneModel, MemoryStorage, MemoryStorageOptions, SessionMetadata,
        StorageBackedSession,
    };
    use crate::agent_core::harness::{background_context, DEFAULT_COMPACTION_SETTINGS};
    use crate::agent_core::types::{QueueMode, ThinkingLevel, ToolExecutionMode};
    use crate::ai::models::faux::{faux_provider, FauxProviderOptions};
    use crate::ai::models::{create_models, CreateModelsOptions};

    async fn create_lane() -> std::sync::Arc<Lane> {
        let storage = MemoryStorage::new(MemoryStorageOptions::default());
        let sess = Arc::new(StorageBackedSession::new(
            SessionMetadata {
                id: "progress-channel-test".into(),
                created_at: 1,
                storage_version: 1,
                ..Default::default()
            },
            Arc::new(storage),
        ));
        let writes: Vec<Write> = vec![
            set_value(&session::branch_tip("main"), serde_json::Value::Null),
            set_value(
                &session::lane_config("main"),
                session::lane_configuration_value(&LaneConfiguration {
                    model: LaneModel {
                        provider: "faux".to_string(),
                        model_id: "faux-1".to_string(),
                    },
                    thinking_level: ThinkingLevel::Off,
                    active_tool_names: Vec::new(),
                }),
            ),
            set_value(
                &session::lane_state("main"),
                serde_json::json!({"currentOperationId":null,"lastOperationId":null,"inbox":[]}),
            ),
        ];
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
        let state = restore_lane(sess.as_ref(), "main", background_context())
            .await
            .expect("lane restores");
        let emit: EmitBatch = Arc::new(|_events, _context| Box::pin(async { Ok(()) }));
        Lane::new(
            "main",
            sess,
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
                tool_execution: ToolExecutionMode::Parallel,
                entry_projectors: None,
            }),
        )
    }

    #[tokio::test]
    async fn frame_channel_commits_writes_and_seal_stops() {
        let lane = create_lane().await;
        let address = pending_assistant_frames("op1", "r1");
        let channel = open_progress::<serde_json::Value>(
            Arc::clone(&lane),
            background_context(),
            Arc::new(move |frame| append_list(&address, frame.clone())),
            Arc::new(|_state| true),
        );
        channel.write(serde_json::json!({"type": "text", "text": "a"}));
        channel.seal();
        channel.write(serde_json::json!({"type": "text", "text": "after-seal"}));
        channel.drain().await.unwrap();
        let frames = lane
            .command(
                move |_state, reader| {
                    let context = background_context();
                    Box::pin(async move {
                        let frames = read_assistant_frames(reader, "op1", "r1", context).await?;
                        Ok(LaneCommand::Return { result: frames })
                    })
                },
                background_context(),
            )
            .await
            .unwrap();
        assert_eq!(frames.len(), 1, "post-seal write must be ignored");
        assert_eq!(frames[0]["text"], "a");
    }

    #[tokio::test]
    async fn frame_channel_skips_writes_without_ownership() {
        let lane = create_lane().await;
        let drive = Drive::new(
            &crate::agent_core::harness::runtime::drive_pass::DriveOptions {
                operation_id: "op2".to_string(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        );
        // No active operation: the ownership guard rejects every write.
        let channel = open_frame_progress(&lane, &drive, "r1");
        channel.write(crate::ai::frame::AssistantMessageFrame::TextDelta {
            content_index: 0,
            delta: "x".to_string(),
        });
        channel.drain().await.unwrap();
        let frames = lane
            .command(
                move |_state, reader| {
                    let context = background_context();
                    Box::pin(async move {
                        let frames = read_assistant_frames(reader, "op2", "r1", context).await?;
                        Ok(LaneCommand::Return { result: frames })
                    })
                },
                background_context(),
            )
            .await
            .unwrap();
        assert!(frames.is_empty());
    }

    #[tokio::test]
    async fn tool_channel_skips_writes_without_ownership() {
        let lane = create_lane().await;
        let drive = Drive::new(
            &crate::agent_core::harness::runtime::drive_pass::DriveOptions {
                operation_id: "op3".to_string(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        );
        let channel = open_tool_progress(&lane, &drive, "t1", 0, "inv1");
        channel.write(serde_json::json!({"output": "x"}));
        channel.drain().await.unwrap();
        let stored = lane
            .command(
                move |_state, reader| {
                    let context = background_context();
                    Box::pin(async move {
                        let stored = reader
                            .get_value(&pending_tool_output("op3", "inv1"), context)
                            .await?;
                        Ok(LaneCommand::Return {
                            result: stored.is_some(),
                        })
                    })
                },
                background_context(),
            )
            .await
            .unwrap();
        assert!(!stored);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn writes_remain_fifo_and_drain_waits_for_all_accepted_frames() {
        let lane = create_lane().await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let blocker_lane = Arc::clone(&lane);
        let e = Arc::clone(&entered);
        let r = Arc::clone(&release);
        let blocker = tokio::spawn(async move {
            blocker_lane
                .command(
                    move |_, _| {
                        let e = Arc::clone(&e);
                        let r = Arc::clone(&r);
                        Box::pin(async move {
                            e.notify_one();
                            r.notified().await;
                            Ok(LaneCommand::Return { result: () })
                        })
                    },
                    background_context(),
                )
                .await
                .unwrap();
        });
        entered.notified().await;
        let address = pending_assistant_frames("ordered", "response");
        let channel = open_progress::<usize>(
            Arc::clone(&lane),
            background_context(),
            Arc::new(move |index| append_list(&address, serde_json::json!(index))),
            Arc::new(|_| true),
        );
        for index in 0..128 {
            channel.write(index);
        }
        channel.seal();
        channel.write(999);
        let drain = channel.drain();
        tokio::pin!(drain);
        assert!(
            futures::poll!(drain.as_mut()).is_pending(),
            "drain cannot finish while command queue is blocked"
        );
        release.notify_one();
        blocker.await.unwrap();
        drain.await.unwrap();
        channel.drain().await.unwrap();
        let frames = lane
            .command(
                move |_, reader| {
                    Box::pin(async move {
                        Ok(LaneCommand::Return {
                            result: read_assistant_frames(
                                reader,
                                "ordered",
                                "response",
                                background_context(),
                            )
                            .await?,
                        })
                    })
                },
                background_context(),
            )
            .await
            .unwrap();
        assert_eq!(
            frames,
            (0..128)
                .map(|index| serde_json::json!(index))
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn drain_propagates_the_latest_write_failure_on_every_wait() {
        let lane = create_lane().await;
        let address = pending_assistant_frames("closed", "response");
        let channel = open_progress::<Value>(
            Arc::clone(&lane),
            background_context(),
            Arc::new(move |frame| append_list(&address, frame.clone())),
            Arc::new(|_| true),
        );
        lane.seal(
            crate::agent_core::harness::runtime::lane::SealKind::Closed,
            "progress refused",
        );
        channel.write(serde_json::json!({"type":"start"}));
        channel.seal();
        for _ in 0..2 {
            let error = channel.drain().await.unwrap_err();
            assert!(
                format!("{error:#}").contains("progress refused"),
                "{error:#}"
            );
        }
    }
}
