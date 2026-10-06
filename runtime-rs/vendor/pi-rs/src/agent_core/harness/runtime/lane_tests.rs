//! Behavior tests for the runtime lane's queue/append/drive surface (the
//! `runtime/lane.ts` fragments ported in this slice: prompt/skill entry
//! points, steer/followUp queues, append, cancelQueued, requestOperationAbort,
//! and the lane snapshot projection).
//!
//! The public-shell equivalents of several of these paths are exercised
//! through `agent_harness::tests`; these tests pin the runtime lane's own
//! contract against the faux provider, mirroring the fixture style of
//! `pi/packages/agent/test/harness/runtime/harness.test.ts`.

use std::sync::Arc;

use super::{Lane, RuntimeConfig};
use crate::agent_core::harness::agent_harness::AgentLane as _;
use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::hooks::HookRegistry;
use crate::agent_core::harness::runtime::drive_pass::DriveOptions;
use crate::agent_core::harness::runtime::durable::LaneState;
use crate::agent_core::harness::session::types::Storage;
use crate::agent_core::harness::session::{
    LaneConfiguration, LaneModel, MemoryStorage, MemoryStorageOptions, SessionMetadata,
    StorageBackedSession,
};
use crate::agent_core::types::{AgentMessage, ThinkingLevel};
use crate::ai::models::faux::{faux_provider, FauxProviderOptions};
use crate::ai::models::{create_models, CreateModelsOptions};
use crate::ai::types::message::{StringOrBlocks, UserMessage};

async fn create_session(id: &str) -> Arc<StorageBackedSession> {
    Arc::new(StorageBackedSession::new(
        SessionMetadata {
            id: id.to_owned(),
            created_at: 1,
            storage_version: 1,
            ..SessionMetadata::default()
        },
        Arc::new(MemoryStorage::new(MemoryStorageOptions::default())) as Arc<dyn Storage>,
    ))
}

fn noop_hook_registry() -> HookRegistry {
    // The upstream `HookRegistry(reportError)` contract: handler failures are
    // reported asynchronously. These tests assert the lane's own outcomes, so
    // the reporter is a no-op sink.
    HookRegistry::new(Arc::new(
        |_error: anyhow::Error,
         _hook,
         _lane: String,
         _context: crate::agent_core::harness::context::Context| {
            Box::pin(async {}) as futures::future::BoxFuture<'static, ()>
        },
    ))
}

/// Upstream `createFixture`-style lane: faux provider + fresh session + a
/// stateless fault handler + a no-op event sink + upstream-default runtime
/// configuration.
async fn create_lane(session_id: &str) -> Arc<Lane> {
    let faux = faux_provider(FauxProviderOptions::default());
    let mut models = create_models(CreateModelsOptions::default());
    models.set_provider(Arc::clone(&faux.provider));
    let model = faux.get_model(None).expect("faux provider exposes faux-1");
    let session = create_session(session_id).await;
    let configuration = LaneConfiguration {
        model: LaneModel {
            provider: model.provider.clone(),
            model_id: model.id.clone(),
        },
        thinking_level: ThinkingLevel::Off,
        active_tool_names: Vec::new(),
    };
    let state = LaneState {
        tip_id: None,
        configuration,
        inbox: Vec::new(),
        last_operation_id: None,
        operation: None,
    };
    Lane::new(
        "main",
        session,
        models,
        noop_hook_registry(),
        state,
        Arc::new(|cause| cause),
        Arc::new(|_batch, _context| Box::pin(async { Ok(()) })),
        Arc::new(RuntimeConfig::default),
    )
}

fn user_message(text: &str) -> AgentMessage {
    AgentMessage::User(UserMessage {
        content: StringOrBlocks::Text(text.to_owned()),
        timestamp: 1,
    })
}

/// Upstream `append` idle path (`lane.ts:1924-1991`): the committed entry
/// becomes the lane tip.
#[tokio::test]
async fn appends_a_user_message_and_moves_the_tip_while_idle() {
    let lane = create_lane("lane-append").await;
    assert_eq!(lane.state().tip_id, None);

    let id = lane
        .append_message(user_message("hello"), background_context())
        .await
        .expect("append commits");
    assert_eq!(lane.state().tip_id.as_deref(), Some(id.as_str()));

    // A second append chains onto the first.
    let second = lane
        .append_message(user_message("again"), background_context())
        .await
        .expect("second append commits");
    assert_eq!(lane.state().tip_id.as_deref(), Some(second.as_str()));
    let entries = lane
        .find_entries(None, background_context())
        .await
        .expect("entries readable");
    assert_eq!(entries.len(), 2, "both appends persisted");
}

/// A pending assistant message cannot enter the transcript
/// (`lane.ts:1936-1940` guard).
#[tokio::test]
async fn rejects_appending_a_pending_assistant_message() {
    let lane = create_lane("lane-append-pending").await;
    let mut assistant =
        crate::ai::models::faux::faux_assistant_message("partial", Default::default());
    assistant.stop_reason = crate::ai::types::primitives::StopReason::Pending;
    let error = lane
        .append_message(AgentMessage::Assistant(assistant), background_context())
        .await
        .expect_err("pending assistant rejected");
    assert!(
        error.to_string().to_lowercase().contains("pending"),
        "unexpected error: {error}"
    );
}

/// Upstream `followUp` + `cancelQueued` while idle: the queued entry id is
/// returned, `cancelQueued` removes it (`"cancelled"`), and a second cancel
/// reports `"not_found"`.
#[tokio::test]
async fn queues_a_follow_up_then_cancels_it() {
    let lane = create_lane("lane-queue").await;

    let queued = lane
        .follow_up(
            crate::agent_core::harness::agent_harness::QueueInput::Text("next".to_owned()),
            None,
            background_context(),
        )
        .await
        .expect("enqueue runs")
        .expect("enqueue accepted");
    assert!(!queued.entry_id.is_empty());

    let cancelled = lane
        .cancel_queued(&queued.entry_id, background_context())
        .await
        .expect("cancel runs");
    assert_eq!(
        cancelled,
        Ok(crate::agent_core::harness::agent_harness::CancelQueuedOutcome::Cancelled)
    );

    let again = lane
        .cancel_queued(&queued.entry_id, background_context())
        .await
        .expect("second cancel runs");
    assert_eq!(
        again,
        Ok(crate::agent_core::harness::agent_harness::CancelQueuedOutcome::NotFound)
    );
}

/// Upstream `requestOperationAbort` on an idle lane: an operation id that is
/// not the active operation surfaces the operation-mismatch tagged error.
#[tokio::test]
async fn request_abort_on_an_idle_lane_reports_the_mismatch() {
    let lane = create_lane("lane-abort-idle").await;
    let result = lane
        .request_abort("op-absent", background_context())
        .await
        .expect("request_abort resolves");
    let error = result.expect_err("no active operation to abort");
    match error {
        crate::agent_core::harness::agent_harness::TaggedError::OperationMismatch {
            expected_operation_id,
            current_operation_id,
            ..
        } => {
            assert_eq!(expected_operation_id, "op-absent");
            assert_eq!(current_operation_id, None);
        }
        other => panic!("unexpected tagged error: {other:?}"),
    }
}

/// The prompt entry point drives the faux provider through the runtime drive
/// and records the transcript; the snapshot projection reports the lane idle
/// afterwards.
#[tokio::test]
async fn prompt_drives_a_faux_run_and_records_the_transcript() {
    let lane = create_lane("lane-prompt").await;

    let result = lane
        .prompt("hello", None, background_context())
        .await
        .expect("prompt resolves");
    let record = match result.expect("prompt accepted") {
        crate::agent_core::harness::agent_harness::RunOutcome::Record(record) => record,
        other => panic!("unexpected non-settled run outcome: {other:?}"),
    };
    assert!(
        !record.operation_id.is_empty(),
        "the record carries the durable operation id"
    );

    let entries = lane
        .find_entries(None, background_context())
        .await
        .expect("transcript readable");
    let roles: Vec<&str> = entries
        .iter()
        .filter_map(|entry| match entry {
            crate::agent_core::harness::session::Entry::Message { message, .. } => match message {
                AgentMessage::User(_) => Some("user"),
                AgentMessage::Assistant(_) => Some("assistant"),
                _ => None,
            },
            _ => None,
        })
        .collect();
    // `find_entries` defaults to newest-first (upstream `findEntries`).
    assert_eq!(
        roles,
        vec!["assistant", "user"],
        "the run recorded the user prompt and the assistant response"
    );

    let snapshot = lane.capture_lane_snapshot(background_context()).await;
    let snapshot = snapshot.expect("snapshot resolves");
    assert!(snapshot.operation.is_none(), "no operation stays open");
    assert_eq!(snapshot.lane, "main", "the snapshot names its lane");
    assert!(
        snapshot.last_result.is_some(),
        "the settled run is remembered as the lane's last result"
    );
}

/// The dispatcher entry point on a lane with no active operation surfaces
/// the same mismatch channel as the public shell (the settled operation id
/// no longer owns the lane).
#[tokio::test]
async fn drive_with_a_stale_operation_id_is_rejected() {
    let lane = create_lane("lane-drive-stale").await;
    let options = DriveOptions {
        operation_id: "op-absent".to_owned(),
        wait_for_retry: None,
        poll_deferred: None,
    };
    let result = lane
        .drive(&options, background_context())
        .await
        .expect("drive resolves");
    assert!(
        matches!(
            result,
            Err(crate::agent_core::harness::agent_harness::TaggedError::OperationMismatch { .. })
        ),
        "a stale operation id must surface the mismatch error, got {result:?}"
    );
}
