//! Contracts for the `MemorySessionRepo`/`MemorySessionFacade` half of
//! `memory.ts` (`memory.ts:136-453`), read in full as the oracle; the
//! upstream `memory-session-repo.test.ts` / `memory-conformance.test.ts`
//! files are the reference suites for a later byte-level replay pass.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::{MemorySessionRepo, MemorySessionRepoOptions};
use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::session::types::{ForkOptions, Session, SessionReader};
use crate::agent_core::harness::session::{commit::insert_entry, types::NewEntry};

const NOW: i64 = 1_700_000_000_000;

fn repo() -> MemorySessionRepo {
    MemorySessionRepo::new(MemorySessionRepoOptions {
        now: Some(Arc::new(move || {
            // Each call advances one millisecond so generated uuidv7 ids and
            // `createdAt` stamps stay distinct and ordered.
            NOW + tick() * 1_000
        })),
    })
}

fn tick() -> i64 {
    use std::sync::atomic::AtomicI64;
    static TICK: AtomicI64 = AtomicI64::new(0);
    TICK.fetch_add(1, Ordering::SeqCst)
}

async fn create_session(repo: &MemorySessionRepo, id: &str) -> Arc<super::MemorySessionFacade> {
    repo.create(
        crate::agent_core::harness::session::types::SessionCreateOptions {
            id: Some(id.to_string()),
            parent_session_id: None,
        },
        background_context(),
    )
    .await
    .unwrap()
}

fn custom_write(id: &str) -> crate::agent_core::harness::session::types::Write {
    insert_entry(NewEntry::Custom {
        id: id.to_string(),
        parent_id: None,
        custom_type: "note".to_string(),
        data: None,
    })
}

#[tokio::test]
async fn create_metadata_and_insertion_ordered_list() {
    let repo = MemorySessionRepo::new(MemorySessionRepoOptions {
        now: Some(Arc::new(|| NOW)),
    });
    let session = create_session(&repo, "s1").await;
    assert_eq!(session.metadata().id, "s1");
    assert_eq!(session.metadata().storage_version, 1);
    assert_eq!(session.metadata().parent_session_id, None);
    let _ = create_session(&repo, "s2").await;
    let listed = repo.list().unwrap();
    let ids: Vec<&str> = listed.iter().map(|metadata| metadata.id.as_str()).collect();
    assert_eq!(ids, vec!["s1", "s2"]);
    assert_eq!(listed[0].created_at, NOW);
}

#[tokio::test]
async fn create_rejects_duplicate_id_across_pending_and_existing() {
    let repo = repo();
    let _ = create_session(&repo, "s1").await;
    let error = repo
        .create(
            crate::agent_core::harness::session::types::SessionCreateOptions {
                id: Some("s1".to_string()),
                parent_session_id: None,
            },
            background_context(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Session already exists: s1"));
}

#[tokio::test]
async fn open_rejects_unknown_and_double_open_then_reopens_after_facade_close() {
    let repo = repo();
    let session = create_session(&repo, "s1").await;
    let unknown = repo
        .open(
            &crate::agent_core::harness::session::types::SessionMetadata {
                id: "missing".to_string(),
                ..crate::agent_core::harness::session::types::SessionMetadata::default()
            },
            background_context(),
        )
        .await
        .unwrap_err();
    assert!(unknown.to_string().contains("Unknown session: missing"));

    let metadata = session.metadata().clone();
    let error = repo
        .open(&metadata, background_context())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Session is already open: s1"));

    session.close(background_context()).await.unwrap();
    let reopened = repo.open(&metadata, background_context()).await.unwrap();
    assert_eq!(reopened.metadata().id, "s1");
}

#[tokio::test]
async fn facade_close_blocks_operations_but_repo_reopen_stays_usable() {
    let repo = repo();
    let session = create_session(&repo, "s1").await;
    session.close(background_context()).await.unwrap();
    let error = session
        .get_entries(&[], background_context())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Session is closed"));
    // The inner session was not closed by the facade close.
    let reopened = repo
        .open(session.metadata(), background_context())
        .await
        .unwrap();
    reopened
        .get_entries(&[], background_context())
        .await
        .unwrap();
}

#[tokio::test]
async fn delete_requires_closed_session_and_frees_the_id() {
    let repo = repo();
    let session = create_session(&repo, "s1").await;
    let error = repo
        .delete(session.metadata(), background_context())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Session is open: s1"));
    session.close(background_context()).await.unwrap();
    repo.delete(session.metadata(), background_context())
        .await
        .unwrap();
    assert!(repo.list().unwrap().is_empty());
    let error = repo
        .delete(session.metadata(), background_context())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Unknown session: s1"));
}

#[tokio::test]
async fn fork_copies_history_sets_parent_and_rejects_duplicates() {
    let repo = repo();
    let session = create_session(&repo, "source").await;
    session
        .mutate(
            |mutator, context| {
                let writes = vec![custom_write("e1")];
                Box::pin(async move { mutator.commit(writes, context).await.map(|_| ()) })
            },
            background_context(),
        )
        .await
        .unwrap();
    let child = repo
        .fork(
            session.metadata(),
            &ForkOptions::Tree {
                id: Some("child".to_string()),
            },
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(
        child.metadata().parent_session_id.as_deref(),
        Some("source")
    );
    let error = repo
        .fork(
            session.metadata(),
            &ForkOptions::Tree {
                id: Some("child".to_string()),
            },
            background_context(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Session already exists: child"));
    let missing = repo
        .fork(
            &crate::agent_core::harness::session::types::SessionMetadata {
                id: "missing".to_string(),
                ..crate::agent_core::harness::session::types::SessionMetadata::default()
            },
            &ForkOptions::Tree { id: None },
            background_context(),
        )
        .await
        .unwrap_err();
    assert!(missing.to_string().contains("Unknown session: missing"));
    // The fork's storage copied the committed entry (Tree scope).
    let entries = child
        .find_entries(None, background_context())
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
}

#[tokio::test]
async fn repo_close_rejects_later_calls_and_closes_backings() {
    let repo = Arc::new(repo());
    let session = create_session(&repo, "s1").await;
    repo.close(background_context()).await.unwrap();
    assert!(repo.list().is_err());
    let error = repo
        .create(
            crate::agent_core::harness::session::types::SessionCreateOptions {
                id: None,
                parent_session_id: None,
            },
            background_context(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("MemorySessionRepo is closed"));
    // The backing session closed with the repo, even while its facade is open.
    let error = session
        .get_entries(&[], background_context())
        .await
        .unwrap_err();
    assert!(error.to_string().to_lowercase().contains("closed"));
}

#[tokio::test]
async fn facade_close_waits_for_the_open_mutation() {
    let repo = repo();
    let session = create_session(&repo, "s1").await;
    let mutation = session.begin_mutation(background_context()).await.unwrap();
    let mut close = Box::pin(session.close(background_context()));
    for _ in 0..16 {
        assert!(futures::poll!(close.as_mut()).is_pending());
        tokio::task::yield_now().await;
    }
    mutation
        .commit(vec![custom_write("e1")], background_context())
        .await
        .unwrap();
    mutation.end(background_context()).await.unwrap();
    close.await.unwrap();
    let error = session
        .get_entry("e1", background_context())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Session is closed"));
}

#[tokio::test]
async fn wrapped_branch_ops_reject_after_facade_close() {
    let repo = repo();
    let session = create_session(&repo, "s1").await;
    let branch = session
        .create_branch("side", None, background_context())
        .await
        .unwrap();
    session.close(background_context()).await.unwrap();
    let error = branch.get_tip_id(background_context()).await.unwrap_err();
    assert!(error.to_string().contains("Session is closed"));
}

/// Byte replay of `packages/agent/test/harness/memory-session-repo.test.ts`
/// (4 scenarios) and of the `MemorySessionRepo` halves of
/// `packages/agent/test/harness/memory-conformance.test.ts` (lifecycle 4,
/// ownership 1, messages 2, fork behavior 7, fork coordination 3, streaming
/// fork 15 — the conformance case bodies run unmodified upstream via
/// `node --experimental-strip-types` over byte-copied sources; 58/58 pass).
/// Timestamps and generated ids are asserted by the upstream generation rule
/// (uuidv7 stamped at the injected clock) rather than by literal, because the
/// random bits differ per run in both implementations.
mod replay {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use super::super::MemorySessionFacade;
    use super::{MemorySessionRepo, MemorySessionRepoOptions};
    use crate::agent_core::chord_support::Context;
    use crate::agent_core::harness::context::background_context;
    use crate::agent_core::harness::session::commit::{insert_entry, insert_usage};
    use crate::agent_core::harness::session::types::{
        AscDescOrder, EntryQuery, EntryType, ForkOptions, ForkPosition, NewEntry, NewUsageRow,
        Session, SessionCreateOptions,
    };
    use crate::agent_core::harness::session::values::{
        append_list, branch_tip, delete_list, entry_label, lane_config, lane_state, list,
        operation_meta, operation_preparation, operation_result, operation_state,
        operation_tool_args, pending_entry, session_name, set_value, value, ListCursor,
        ListReadOptions,
    };
    use crate::agent_core::types::AgentMessage;
    use crate::ai::types::{
        AssistantBlock, AssistantMessage, DeferredHandle, StopReason, StringOrBlocks, TextContent,
        ToolCall, Usage, UsageCost, UserMessage,
    };

    const NOW: i64 = 1_700_000_000_000;
    const ROOT_ID: &str = "00000000-0000-7000-8000-000000000001";
    const CHILD_ID: &str = "00000000-0000-7000-8000-000000000002";
    const SIBLING_ID: &str = "00000000-0000-7000-8000-000000000003";
    const USAGE_ID: &str = "00000000-0000-7000-8000-000000000004";
    const OPERATION_ID: &str = "00000000-0000-7000-8000-000000000005";
    const PENDING_ID: &str = "00000000-0000-7000-8000-000000000006";
    const UNKNOWN_ID: &str = "00000000-0000-7000-8000-000000000007";

    fn ctx() -> Context {
        background_context()
    }

    /// Upstream `new MemorySessionRepo({ now: () => NOW })`.
    fn fixed_repo() -> MemorySessionRepo {
        MemorySessionRepo::new(MemorySessionRepoOptions {
            now: Some(Arc::new(|| NOW)),
        })
    }

    async fn create(repo: &MemorySessionRepo, id: &str) -> Arc<MemorySessionFacade> {
        repo.create(
            SessionCreateOptions {
                id: Some(id.to_string()),
                parent_session_id: None,
            },
            ctx(),
        )
        .await
        .unwrap()
    }

    async fn commit_writes(
        session: &MemorySessionFacade,
        writes: Vec<crate::agent_core::harness::session::types::Write>,
    ) {
        session
            .mutate(
                |mutator, context| {
                    Box::pin(async move { mutator.commit(writes, context).await.map(|_| ()) })
                },
                ctx(),
            )
            .await
            .unwrap();
    }

    /// Upstream `getBranchTip` for a branch that must exist.
    async fn tip_of(session: &MemorySessionFacade, name: &str) -> Option<String> {
        let branch = session
            .branch(name, ctx())
            .await
            .unwrap()
            .expect("branch present");
        branch.get_tip_id(ctx()).await.unwrap()
    }

    fn configuration() -> serde_json::Value {
        serde_json::json!({
            "model": { "provider": "provider", "modelId": "model" },
            "thinkingLevel": "off",
            "activeToolNames": ["read"],
        })
    }

    fn idle_lane_state() -> serde_json::Value {
        serde_json::json!({
            "currentOperationId": serde_json::Value::Null,
            "lastOperationId": serde_json::Value::Null,
            "inbox": [],
        })
    }

    fn custom_entry(
        id: &str,
        parent_id: Option<&str>,
        custom_type: &str,
    ) -> crate::agent_core::harness::session::types::Write {
        insert_entry(NewEntry::Custom {
            id: id.to_string(),
            parent_id: parent_id.map(str::to_string),
            custom_type: custom_type.to_string(),
            data: None,
        })
    }

    fn message_entry(
        id: &str,
        parent_id: &str,
        text: &str,
    ) -> crate::agent_core::harness::session::types::Write {
        insert_entry(NewEntry::Message {
            id: id.to_string(),
            parent_id: Some(parent_id.to_string()),
            message: AgentMessage::User(UserMessage {
                content: StringOrBlocks::Text(text.to_string()),
                timestamp: 1,
            }),
            terminate: None,
        })
    }

    fn usage_row() -> NewUsageRow {
        NewUsageRow {
            id: USAGE_ID.to_string(),
            usage: Usage {
                input: 1,
                output: 2,
                cache_read: 0,
                cache_write: 0,
                cache_write_1h: None,
                reasoning: None,
                total_tokens: 3,
                cost: UsageCost::default(),
            },
            entry_id: None,
            adjustment: true,
            details: None,
        }
    }

    fn stop_reason_text(stop_reason: StopReason) -> &'static str {
        match stop_reason {
            StopReason::Stop => "stop",
            StopReason::Length => "length",
            StopReason::ToolUse => "toolUse",
            StopReason::Error => "error",
            StopReason::Aborted => "aborted",
            StopReason::Deferred => "deferred",
            StopReason::Pending => "pending",
        }
    }

    /// Upstream `assistantMessage(stopReason)` (`conformance/session-repo.ts:58-89`).
    fn assistant_message(stop_reason: StopReason) -> AgentMessage {
        let content = if stop_reason == StopReason::ToolUse {
            vec![AssistantBlock::ToolCall(ToolCall {
                id: "call".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({}),
                thought_signature: None,
                namespace: None,
            })]
        } else {
            vec![AssistantBlock::Text(TextContent {
                text: stop_reason_text(stop_reason).to_string(),
                text_signature: None,
            })]
        };
        AgentMessage::Assistant(AssistantMessage {
            content,
            api: "anthropic-messages".to_string(),
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason,
            deferred: (stop_reason == StopReason::Deferred).then(|| DeferredHandle {
                provider: "anthropic".to_string(),
                model_id: "claude-sonnet-4-5".to_string(),
                api: "anthropic-messages".to_string(),
                id: "job".to_string(),
                expires_at: None,
                poll_after_ms: None,
                data: None,
            }),
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 1,
        })
    }

    // === memory-session-repo.test.ts ========================================

    /// "uses its injected clock for generated session identity and metadata".
    #[tokio::test]
    async fn replay_clock_generates_session_identity_from_injected_clock() {
        let repo = fixed_repo();
        let session = repo
            .create(SessionCreateOptions::default(), ctx())
            .await
            .unwrap();

        // expect(session.metadata.createdAt).toBe(NOW)
        assert_eq!(session.metadata().created_at, NOW);
        // expect(uuidTimestamp(session.metadata.id)).toBe(NOW) — generated ids
        // are asserted by the upstream uuidv7-at-timestamp rule, not literally.
        let id = session.metadata().id.clone();
        let stamp = i64::from_str_radix(&id.replace('-', "")[..12], 16).unwrap();
        assert_eq!(stamp, NOW);
        assert_eq!(session.metadata().storage_version, 1);
        session.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    /// "returns a fresh facade after close while retaining one session and
    /// storage".
    #[tokio::test]
    async fn replay_fresh_facade_after_close_retains_one_session_and_storage() {
        let repo = fixed_repo();
        let first = create(&repo, "session").await;
        let first_branch = first.create_branch("main", None, ctx()).await.unwrap();
        // Admitted but not awaited before the double-open rejection.
        let admitted_write = first.set_name(Some("preserved".to_string()), ctx());

        let error = repo.open(first.metadata(), ctx()).await.unwrap_err();
        assert!(error.to_string().contains("already open"), "{error}");
        admitted_write.await.unwrap();
        first.close(ctx()).await.unwrap();
        let error = first.get_name(ctx()).await.unwrap_err();
        assert!(error.to_string().contains("Session is closed"), "{error}");
        let error = first
            .scan_branch(
                &crate::agent_core::harness::session::types::StorageBranchScan {
                    start: "entry".to_string(),
                    ..Default::default()
                },
                ctx(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Session is closed"), "{error}");
        let error = first_branch.get_tip_id(ctx()).await.unwrap_err();
        assert!(error.to_string().contains("Session is closed"), "{error}");

        let second = repo.open(first.metadata(), ctx()).await.unwrap();
        // expect(second).not.toBe(first)
        assert!(!Arc::ptr_eq(&second, &first));
        assert_eq!(
            second.get_name(ctx()).await.unwrap().as_deref(),
            Some("preserved")
        );
        second.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    /// "waits for an explicit mutation before closing its facade".
    #[tokio::test]
    async fn replay_close_waits_for_explicit_mutation() {
        let repo = fixed_repo();
        let session = create(&repo, "session").await;
        let mutation = session.begin_mutation(ctx()).await.unwrap();
        let mut closing = Box::pin(session.close(ctx()));

        // await Promise.resolve(); expect(closed).toBe(false)
        for _ in 0..16 {
            assert!(futures::poll!(closing.as_mut()).is_pending());
            tokio::task::yield_now().await;
        }
        mutation.end(ctx()).await.unwrap();
        closing.await.unwrap();

        let reopened = repo.open(session.metadata(), ctx()).await.unwrap();
        reopened.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    /// "rejects an explicit scope that had not acquired before facade close".
    #[tokio::test]
    async fn replay_rejects_scope_not_acquired_before_facade_close() {
        let repo = fixed_repo();
        let session = create(&repo, "session").await;
        let first = session.begin_mutation(ctx()).await.unwrap();
        let mut second = Box::pin(session.begin_mutation(ctx()));
        // The queued scope parks on the mutation line, still admitted.
        assert!(futures::poll!(second.as_mut()).is_pending());
        let mut closing = Box::pin(session.close(ctx()));
        assert!(futures::poll!(closing.as_mut()).is_pending());

        first.end(ctx()).await.unwrap();
        let error = match second.await {
            Ok(_) => panic!("queued scope must reject after close"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("Session is closed"), "{error}");
        closing.await.unwrap();

        let reopened = repo.open(session.metadata(), ctx()).await.unwrap();
        reopened.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    // === MemorySessionRepo conformance: lifecycle ===========================

    /// "creates a session with no implicit branch and rejects duplicate ids".
    #[tokio::test]
    async fn replay_creates_session_no_implicit_branch_rejects_duplicate_ids() {
        let repo = fixed_repo();
        let session = create(&repo, "session").await;

        assert_eq!(session.metadata().id, "session");
        // Number.isSafeInteger(createdAt) with the injected clock is exactly NOW.
        assert_eq!(session.metadata().created_at, NOW);
        assert_eq!(session.metadata().storage_version, 1);
        assert!(session.branch("main", ctx()).await.unwrap().is_none());
        assert!(session
            .get_value(&lane_state("main"), ctx())
            .await
            .unwrap()
            .is_none());
        assert!(session
            .get_value(&lane_config("main"), ctx())
            .await
            .unwrap()
            .is_none());
        let error = repo
            .create(
                SessionCreateOptions {
                    id: Some("session".to_string()),
                    parent_session_id: None,
                },
                ctx(),
            )
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Session already exists: session"),
            "{error}"
        );
        session.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    /// "close drains an acquired scope and rejects a queued mutation callback".
    #[tokio::test]
    async fn replay_close_drains_acquired_scope_rejects_queued_mutation() {
        let repo = fixed_repo();
        let session = create(&repo, "session").await;
        let active = session.begin_mutation(ctx()).await.unwrap();
        let queued_started = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&queued_started);
        let mut queued = Box::pin(session.mutate(
            move |_mutator, _context| {
                flag.store(true, Ordering::SeqCst);
                Box::pin(async { Ok(()) })
            },
            ctx(),
        ));
        // Queued behind the acquired mutation line.
        assert!(futures::poll!(queued.as_mut()).is_pending());
        let mut closing = Box::pin(session.close(ctx()));
        assert!(futures::poll!(closing.as_mut()).is_pending());

        active.end(ctx()).await.unwrap();
        let error = match queued.await {
            Ok(()) => panic!("queued callback must reject after close"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("Session is closed"), "{error}");
        closing.await.unwrap();
        assert!(!queued_started.load(Ordering::SeqCst));
        repo.close(ctx()).await.unwrap();
    }

    /// "lists metadata and preserves state across close and reopen".
    #[tokio::test]
    async fn replay_lists_metadata_preserves_state_across_close_and_reopen() {
        let repo = fixed_repo();
        let first = create(&repo, "first").await;
        first
            .set_name(Some("preserved".to_string()), ctx())
            .await
            .unwrap();
        let second = repo
            .create(
                SessionCreateOptions {
                    id: Some("second".to_string()),
                    parent_session_id: Some("parent".to_string()),
                },
                ctx(),
            )
            .await
            .unwrap();
        second.close(ctx()).await.unwrap();

        let listed = repo.list().unwrap();
        let shaped: Vec<(&str, Option<&str>)> = listed
            .iter()
            .map(|metadata| (metadata.id.as_str(), metadata.parent_session_id.as_deref()))
            .collect();
        assert_eq!(shaped, vec![("first", None), ("second", Some("parent"))]);
        first.close(ctx()).await.unwrap();
        let error = first.get_name(ctx()).await.unwrap_err();
        assert!(error.to_string().contains("Session is closed"), "{error}");
        let reopened = repo.open(first.metadata(), ctx()).await.unwrap();
        assert!(!Arc::ptr_eq(&reopened, &first));
        assert_eq!(
            reopened.get_name(ctx()).await.unwrap().as_deref(),
            Some("preserved")
        );
        reopened.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    /// "deletes closed sessions without affecting other sessions".
    #[tokio::test]
    async fn replay_deletes_closed_sessions_without_affecting_others() {
        let repo = fixed_repo();
        let removed = create(&repo, "removed").await;
        let retained = create(&repo, "retained").await;
        removed.close(ctx()).await.unwrap();
        retained.close(ctx()).await.unwrap();

        repo.delete(removed.metadata(), ctx()).await.unwrap();
        let listed = repo.list().unwrap();
        assert_eq!(
            listed.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["retained"]
        );
        assert!(repo.open(removed.metadata(), ctx()).await.is_err());
        assert!(repo.delete(removed.metadata(), ctx()).await.is_err());
        repo.close(ctx()).await.unwrap();
    }

    // === MemorySessionRepo conformance: ownership ===========================

    /// "rejects opening an already-open session".
    #[tokio::test]
    async fn replay_rejects_opening_already_open_session() {
        let repo = fixed_repo();
        let session = create(&repo, "session").await;
        assert!(repo.open(session.metadata(), ctx()).await.is_err());
        session.close(ctx()).await.unwrap();

        let reopened = repo.open(session.metadata(), ctx()).await.unwrap();
        assert!(repo.open(session.metadata(), ctx()).await.is_err());
        reopened.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    // === MemorySessionRepo conformance: messages ============================

    /// "rejects pending assistant messages without changing the tree".
    #[tokio::test]
    async fn replay_rejects_pending_assistant_messages_without_changing_tree() {
        let repo = fixed_repo();
        let session = create(&repo, "session").await;
        let branch = session.create_branch("main", None, ctx()).await.unwrap();

        assert!(branch
            .append_message(assistant_message(StopReason::Pending), ctx())
            .await
            .is_err());

        assert_eq!(tip_of(&session, "main").await, None);
        assert!(session.find_entries(None, ctx()).await.unwrap().is_empty());
        session.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    /// "preserves every settled assistant stop reason".
    #[tokio::test]
    async fn replay_preserves_every_settled_assistant_stop_reason() {
        let repo = fixed_repo();
        let session = create(&repo, "session").await;
        let messages = [
            assistant_message(StopReason::Stop),
            assistant_message(StopReason::Length),
            assistant_message(StopReason::ToolUse),
            assistant_message(StopReason::Error),
            assistant_message(StopReason::Aborted),
            assistant_message(StopReason::Deferred),
        ];
        let branch = session.create_branch("main", None, ctx()).await.unwrap();
        let mut ids = Vec::new();
        for message in &messages {
            ids.push(branch.append_message(message.clone(), ctx()).await.unwrap());
        }

        let entries = session
            .find_entries(
                Some(&EntryQuery {
                    order: Some(AscDescOrder::Asc),
                    scan_type: Some(EntryType::Message),
                    ..Default::default()
                }),
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.id().to_string())
                .collect::<Vec<_>>(),
            ids
        );
        for (index, entry) in entries.iter().enumerate() {
            assert_eq!(entry.message(), Some(&messages[index]));
        }
        assert_eq!(
            tip_of(&session, "main").await.as_deref(),
            ids.last().map(String::as_str)
        );
        session.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    // === MemorySessionRepo conformance: forks ===============================

    /// "tree-forks a fresh session before first attachment".
    #[tokio::test]
    async fn replay_tree_forks_fresh_session_before_first_attachment() {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        let fork = repo
            .fork(
                source.metadata(),
                &ForkOptions::Tree {
                    id: Some("fork".to_string()),
                },
                ctx(),
            )
            .await
            .unwrap();

        assert_eq!(fork.metadata().id, "fork");
        assert_eq!(fork.metadata().parent_session_id.as_deref(), Some("source"));
        assert!(fork.branch("main", ctx()).await.unwrap().is_none());
        assert!(fork
            .get_value(&lane_config("main"), ctx())
            .await
            .unwrap()
            .is_none());
        assert!(fork
            .get_value(&lane_state("main"), ctx())
            .await
            .unwrap()
            .is_none());
        assert!(fork.find_entries(None, ctx()).await.unwrap().is_empty());
        let stats = fork.get_stats(ctx()).await.unwrap();
        assert_eq!(stats.message_count, 0);
        assert_eq!(stats.usage, Usage::default());
        source.close(ctx()).await.unwrap();
        fork.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    /// "rejects a data-only branch and releases its destination id".
    #[tokio::test]
    async fn replay_rejects_data_only_branch_and_releases_destination_id() {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        source.create_branch("data", None, ctx()).await.unwrap();

        assert!(repo
            .fork(
                source.metadata(),
                &ForkOptions::Branch {
                    branch: "data".to_string(),
                    entry_id: None,
                    position: None,
                    id: Some("destination".to_string()),
                },
                ctx(),
            )
            .await
            .is_err());
        let listed = repo.list().unwrap();
        assert_eq!(
            listed.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["source"]
        );

        // The destination id was released: create may reuse it.
        let destination = create(&repo, "destination").await;
        source.close(ctx()).await.unwrap();
        destination.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    /// The large single-commit fixture of "forks one named configured branch
    /// with scoped values and a zero ledger".
    fn configured_branch_writes() -> Vec<crate::agent_core::harness::session::types::Write> {
        vec![
            custom_entry(ROOT_ID, None, "root"),
            message_entry(CHILD_ID, ROOT_ID, "child"),
            custom_entry(SIBLING_ID, Some(ROOT_ID), "sibling"),
            set_value(&branch_tip("main"), serde_json::json!(SIBLING_ID)),
            set_value(&branch_tip("review"), serde_json::json!(CHILD_ID)),
            set_value(&lane_config("review"), configuration()),
            set_value(
                &lane_state("review"),
                serde_json::json!({
                    "currentOperationId": OPERATION_ID,
                    "lastOperationId": "previous",
                    "inbox": [{ "entryId": PENDING_ID, "kind": "write" }],
                }),
            ),
            set_value(
                &operation_result("previous"),
                serde_json::json!({
                    "operationId": "previous",
                    "kind": "navigation",
                    "status": "completed",
                    "fromTipId": ROOT_ID,
                    "tipId": CHILD_ID,
                    "startedAt": 1,
                    "endedAt": 2,
                }),
            ),
            set_value(&session_name(), serde_json::json!("source name")),
            set_value(
                &value("test.application.value", ""),
                serde_json::json!({ "copied": false }),
            ),
            append_list(
                &list("test.application.list", ""),
                serde_json::json!({ "copied": false }),
            ),
            set_value(&entry_label(ROOT_ID), serde_json::json!("root label")),
            set_value(&entry_label(SIBLING_ID), serde_json::json!("sibling label")),
            set_value(
                &pending_entry(PENDING_ID),
                serde_json::json!({ "type": "custom", "customType": "pending" }),
            ),
            set_value(
                &operation_meta(OPERATION_ID),
                serde_json::json!({
                    "operationId": OPERATION_ID,
                    "lane": "review",
                    "sourceTipId": CHILD_ID,
                    "startedAt": 1,
                    "intent": { "kind": "compaction" },
                }),
            ),
            set_value(
                &operation_state(OPERATION_ID),
                serde_json::json!({
                    "at": "summary.deciding",
                    "control": { "status": "running" },
                    "settings": {
                        "compaction": { "enabled": true, "reserveTokens": 1, "keepRecentTokens": 1 },
                        "steeringMode": "all",
                        "followUpMode": "all",
                        "toolExecution": "sequential",
                    },
                    "latestAssistantEntryId": serde_json::Value::Null,
                    "task": { "taskId": OPERATION_ID, "reason": "manual", "boundary": { "kind": "finish" } },
                }),
            ),
            set_value(
                &operation_tool_args(OPERATION_ID, ROOT_ID, 0),
                serde_json::json!({ "argument": true }),
            ),
            set_value(
                &operation_preparation(OPERATION_ID, OPERATION_ID),
                serde_json::json!({
                    "kind": "compaction",
                    "messagesToSummarize": [],
                    "turnPrefixMessages": [],
                    "retainedTail": [],
                    "isSplitTurn": false,
                    "tokensBefore": 0,
                    "fileOps": { "read": [], "written": [], "edited": [] },
                    "settings": { "enabled": true, "reserveTokens": 1, "keepRecentTokens": 1 },
                }),
            ),
            insert_usage(usage_row()),
        ]
    }

    /// "forks one named configured branch with scoped values and a zero ledger".
    #[tokio::test]
    async fn replay_forks_named_configured_branch_scoped_values_zero_ledger() {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        commit_writes(&source, configured_branch_writes()).await;

        let fork = repo
            .fork(
                source.metadata(),
                &ForkOptions::Branch {
                    branch: "review".to_string(),
                    entry_id: Some(CHILD_ID.to_string()),
                    position: Some(ForkPosition::At),
                    id: Some("fork".to_string()),
                },
                ctx(),
            )
            .await
            .unwrap();

        let entries = fork
            .find_entries(
                Some(&EntryQuery {
                    order: Some(AscDescOrder::Asc),
                    ..Default::default()
                }),
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.id().to_string())
                .collect::<Vec<_>>(),
            vec![ROOT_ID, CHILD_ID]
        );
        assert!(fork.branch("main", ctx()).await.unwrap().is_none());
        assert_eq!(tip_of(&fork, "review").await.as_deref(), Some(CHILD_ID));
        assert_eq!(
            fork.get_value(&lane_config("review"), ctx())
                .await
                .unwrap()
                .unwrap()
                .value,
            configuration()
        );
        assert_eq!(
            fork.get_value(&lane_state("review"), ctx())
                .await
                .unwrap()
                .unwrap()
                .value,
            idle_lane_state()
        );
        assert!(fork
            .get_value(&lane_config("main"), ctx())
            .await
            .unwrap()
            .is_none());
        assert!(fork
            .get_value(&lane_state("main"), ctx())
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            fork.get_name(ctx()).await.unwrap().as_deref(),
            Some("source name")
        );
        assert!(fork
            .get_value(&value("test.application.value", ""), ctx())
            .await
            .unwrap()
            .is_none());
        assert!(fork
            .read_list(&list("test.application.list", ""), None, ctx())
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            fork.get_label(ROOT_ID, ctx()).await.unwrap().as_deref(),
            Some("root label")
        );
        assert_eq!(fork.get_label(SIBLING_ID, ctx()).await.unwrap(), None);
        for address in [
            operation_result("previous"),
            pending_entry(PENDING_ID),
            operation_meta(OPERATION_ID),
            operation_state(OPERATION_ID),
            operation_tool_args(OPERATION_ID, ROOT_ID, 0),
            operation_preparation(OPERATION_ID, OPERATION_ID),
        ] {
            assert!(
                fork.get_value(&address, ctx()).await.unwrap().is_none(),
                "{address:?}"
            );
        }
        let stats = fork.get_stats(ctx()).await.unwrap();
        assert_eq!(stats.message_count, 1);
        assert_eq!(stats.usage, Usage::default());
        source.close(ctx()).await.unwrap();
        fork.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    /// "enforces branch ancestry for at and before placement".
    #[tokio::test]
    async fn replay_enforces_branch_ancestry_for_at_and_before_placement() {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        commit_writes(
            &source,
            vec![
                custom_entry(ROOT_ID, None, "root"),
                custom_entry(CHILD_ID, Some(ROOT_ID), "child"),
                custom_entry(SIBLING_ID, Some(ROOT_ID), "sibling"),
                set_value(&branch_tip("main"), serde_json::json!(CHILD_ID)),
                set_value(&lane_config("main"), configuration()),
                set_value(&lane_state("main"), idle_lane_state()),
                set_value(&branch_tip("empty"), serde_json::Value::Null),
                set_value(&lane_config("empty"), configuration()),
                set_value(&lane_state("empty"), idle_lane_state()),
            ],
        )
        .await;

        let before = repo
            .fork(
                source.metadata(),
                &ForkOptions::Branch {
                    branch: "main".to_string(),
                    entry_id: Some(CHILD_ID.to_string()),
                    position: Some(ForkPosition::Before),
                    id: Some("before".to_string()),
                },
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(tip_of(&before, "main").await.as_deref(), Some(ROOT_ID));
        assert_eq!(ancestry_ids(&before).await, vec![ROOT_ID.to_string()]);

        let mid = repo
            .fork(
                source.metadata(),
                &ForkOptions::Branch {
                    branch: "main".to_string(),
                    entry_id: Some(ROOT_ID.to_string()),
                    position: Some(ForkPosition::At),
                    id: Some("mid".to_string()),
                },
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(tip_of(&mid, "main").await.as_deref(), Some(ROOT_ID));
        assert_eq!(ancestry_ids(&mid).await, vec![ROOT_ID.to_string()]);

        let before_root = repo
            .fork(
                source.metadata(),
                &ForkOptions::Branch {
                    branch: "main".to_string(),
                    entry_id: Some(ROOT_ID.to_string()),
                    position: Some(ForkPosition::Before),
                    id: Some("before-root".to_string()),
                },
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(tip_of(&before_root, "main").await, None);
        assert!(ancestry_ids(&before_root).await.is_empty());

        let empty = repo
            .fork(
                source.metadata(),
                &ForkOptions::Branch {
                    branch: "empty".to_string(),
                    entry_id: None,
                    position: None,
                    id: Some("empty".to_string()),
                },
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(tip_of(&empty, "empty").await, None);
        assert!(ancestry_ids(&empty).await.is_empty());

        for (id, branch, entry_id) in [
            ("off-branch", "main", SIBLING_ID),
            ("unknown", "main", UNKNOWN_ID),
            ("null-tip", "empty", ROOT_ID),
        ] {
            assert!(
                repo.fork(
                    source.metadata(),
                    &ForkOptions::Branch {
                        branch: branch.to_string(),
                        entry_id: Some(entry_id.to_string()),
                        position: None,
                        id: Some(id.to_string()),
                    },
                    ctx(),
                )
                .await
                .is_err(),
                "expected fork {id} to reject"
            );
        }
        let mut listed = repo
            .list()
            .unwrap()
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>();
        listed.sort();
        assert_eq!(
            listed,
            vec!["before", "before-root", "empty", "mid", "source"]
        );
        source.close(ctx()).await.unwrap();
        before.close(ctx()).await.unwrap();
        before_root.close(ctx()).await.unwrap();
        empty.close(ctx()).await.unwrap();
        mid.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    async fn ancestry_ids(session: &MemorySessionFacade) -> Vec<String> {
        session
            .find_entries(
                Some(&EntryQuery {
                    order: Some(AscDescOrder::Asc),
                    ..Default::default()
                }),
                ctx(),
            )
            .await
            .unwrap()
            .iter()
            .map(|entry| entry.id().to_string())
            .collect()
    }

    /// "forks a closed source session".
    #[tokio::test]
    async fn replay_forks_closed_source_session() {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        commit_writes(
            &source,
            vec![
                custom_entry(ROOT_ID, None, "root"),
                set_value(&branch_tip("main"), serde_json::json!(ROOT_ID)),
                set_value(&lane_config("main"), configuration()),
                set_value(&lane_state("main"), idle_lane_state()),
                set_value(
                    &value("test.application.value", ""),
                    serde_json::json!("excluded"),
                ),
                append_list(
                    &list("test.application.list", ""),
                    serde_json::json!("excluded"),
                ),
            ],
        )
        .await;
        source.close(ctx()).await.unwrap();

        let fork = repo
            .fork(
                source.metadata(),
                &ForkOptions::Branch {
                    branch: "main".to_string(),
                    entry_id: None,
                    position: None,
                    id: Some("fork".to_string()),
                },
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(tip_of(&fork, "main").await.as_deref(), Some(ROOT_ID));
        assert_eq!(
            fork.get_value(&lane_config("main"), ctx())
                .await
                .unwrap()
                .unwrap()
                .value,
            configuration()
        );
        assert_eq!(
            fork.get_value(&lane_state("main"), ctx())
                .await
                .unwrap()
                .unwrap()
                .value,
            idle_lane_state()
        );
        assert!(fork
            .get_value(&value("test.application.value", ""), ctx())
            .await
            .unwrap()
            .is_none());
        assert!(fork
            .read_list(&list("test.application.list", ""), None, ctx())
            .await
            .unwrap()
            .is_empty());
        fork.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    /// "forks the whole configured tree with fresh lane state".
    #[tokio::test]
    async fn replay_forks_whole_configured_tree_with_fresh_lane_state() {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        commit_writes(
            &source,
            vec![
                custom_entry(ROOT_ID, None, "root"),
                custom_entry(CHILD_ID, Some(ROOT_ID), "child"),
                custom_entry(SIBLING_ID, Some(ROOT_ID), "sibling"),
                set_value(&branch_tip("main"), serde_json::json!(CHILD_ID)),
                set_value(&lane_config("main"), configuration()),
                set_value(&lane_state("main"), idle_lane_state()),
                set_value(&branch_tip("review"), serde_json::json!(SIBLING_ID)),
                set_value(&lane_config("review"), configuration()),
                set_value(&lane_state("review"), idle_lane_state()),
                set_value(&branch_tip("notes"), serde_json::json!(ROOT_ID)),
                set_value(
                    &value("test.application.value", ""),
                    serde_json::json!({ "copied": true }),
                ),
            ],
        )
        .await;

        let fork = repo
            .fork(
                source.metadata(),
                &ForkOptions::Tree {
                    id: Some("fork".to_string()),
                },
                ctx(),
            )
            .await
            .unwrap();

        assert_eq!(
            ancestry_ids(&fork).await,
            vec![
                ROOT_ID.to_string(),
                CHILD_ID.to_string(),
                SIBLING_ID.to_string()
            ]
        );
        assert_eq!(tip_of(&fork, "main").await.as_deref(), Some(CHILD_ID));
        assert_eq!(tip_of(&fork, "review").await.as_deref(), Some(SIBLING_ID));
        assert_eq!(tip_of(&fork, "notes").await.as_deref(), Some(ROOT_ID));
        assert!(fork
            .get_value(&lane_config("notes"), ctx())
            .await
            .unwrap()
            .is_none());
        assert!(fork
            .get_value(&lane_state("notes"), ctx())
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            fork.get_value(&lane_config("review"), ctx())
                .await
                .unwrap()
                .unwrap()
                .value,
            configuration()
        );
        assert_eq!(
            fork.get_value(&lane_state("review"), ctx())
                .await
                .unwrap()
                .unwrap()
                .value,
            idle_lane_state()
        );
        assert_eq!(
            fork.get_value(&value("test.application.value", ""), ctx())
                .await
                .unwrap()
                .unwrap()
                .value,
            serde_json::json!({ "copied": true })
        );
        source.close(ctx()).await.unwrap();
        fork.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    /// "rejects only surviving unknown reserved scalar state".
    #[tokio::test]
    async fn replay_rejects_only_surviving_unknown_reserved_scalar_state() {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        source.create_branch("main", None, ctx()).await.unwrap();
        commit_writes(
            &source,
            vec![
                set_value(&lane_config("main"), configuration()),
                set_value(&lane_state("main"), idle_lane_state()),
            ],
        )
        .await;

        for namespace in ["pi", "pi.unknown"] {
            let address = value(namespace, "");
            source
                .set_value(address.clone(), serde_json::json!(true), ctx())
                .await
                .unwrap();
            assert!(
                repo.fork(
                    source.metadata(),
                    &ForkOptions::Tree {
                        id: Some("tree".to_string())
                    },
                    ctx(),
                )
                .await
                .is_err(),
                "tree fork with surviving {namespace} must reject"
            );
            assert!(
                repo.fork(
                    source.metadata(),
                    &ForkOptions::Branch {
                        branch: "main".to_string(),
                        entry_id: None,
                        position: None,
                        id: Some("branch".to_string()),
                    },
                    ctx(),
                )
                .await
                .is_err(),
                "branch fork with surviving {namespace} must reject"
            );
            source.delete_value(address, ctx()).await.unwrap();
        }
        source.close(ctx()).await.unwrap();

        let tree = repo
            .fork(
                source.metadata(),
                &ForkOptions::Tree {
                    id: Some("tree".to_string()),
                },
                ctx(),
            )
            .await
            .unwrap();
        let branch = repo
            .fork(
                source.metadata(),
                &ForkOptions::Branch {
                    branch: "main".to_string(),
                    entry_id: None,
                    position: None,
                    id: Some("branch".to_string()),
                },
                ctx(),
            )
            .await
            .unwrap();
        tree.close(ctx()).await.unwrap();
        branch.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    // === MemorySessionRepo conformance: fork coordination ===================

    /// "publishes create when it reserves a shared destination id first".
    /// Upstream reaches this deterministically because `create` reserves
    /// synchronously before `fork` is invoked; the replay awaits the winner
    /// first (disclosed sequentialization of `Promise.allSettled`).
    #[tokio::test]
    async fn replay_publishes_create_reserving_shared_destination_first() {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        let destination = create(&repo, "destination").await;
        assert!(repo
            .fork(
                source.metadata(),
                &ForkOptions::Tree {
                    id: Some("destination".to_string())
                },
                ctx(),
            )
            .await
            .is_err());
        destination.close(ctx()).await.unwrap();
        source.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    /// "publishes fork when it reserves a shared destination id first"
    /// (see the sequentialization note above).
    #[tokio::test]
    async fn replay_publishes_fork_reserving_shared_destination_first() {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        let fork = repo
            .fork(
                source.metadata(),
                &ForkOptions::Tree {
                    id: Some("destination".to_string()),
                },
                ctx(),
            )
            .await
            .unwrap();
        assert!(repo
            .create(
                SessionCreateOptions {
                    id: Some("destination".to_string()),
                    parent_session_id: None,
                },
                ctx(),
            )
            .await
            .is_err());
        fork.close(ctx()).await.unwrap();
        source.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    /// "captures one coherent boundary between source commits".
    #[tokio::test]
    async fn replay_captures_coherent_boundary_between_source_commits() {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        let first_mutation = source.begin_mutation(ctx()).await.unwrap();
        let first_commit = first_mutation.commit(
            vec![
                custom_entry(ROOT_ID, None, "first"),
                set_value(&branch_tip("main"), serde_json::json!(ROOT_ID)),
                set_value(&lane_config("main"), configuration()),
                set_value(&lane_state("main"), idle_lane_state()),
                set_value(&session_name(), serde_json::json!("first name")),
                set_value(&entry_label(ROOT_ID), serde_json::json!("first label")),
            ],
            ctx(),
        );
        let fork_options = fork_branch("main", None, Some("fork"));
        let fork_future = repo.fork(source.metadata(), &fork_options, ctx());
        // Polled in call order: the first commit lands, then the fork
        // snapshots that boundary.
        let (first_commit, forked) = tokio::join!(first_commit, fork_future);
        first_commit.unwrap();
        let forked = forked.unwrap();

        let second_commit = source.mutate(
            |mutator, context| {
                Box::pin(async move {
                    mutator
                        .commit(
                            vec![
                                custom_entry(CHILD_ID, Some(ROOT_ID), "second"),
                                set_value(&branch_tip("main"), serde_json::json!(CHILD_ID)),
                                set_value(&session_name(), serde_json::json!("second name")),
                                set_value(&entry_label(ROOT_ID), serde_json::json!("second label")),
                            ],
                            context,
                        )
                        .await
                        .map(|_| ())
                })
            },
            ctx(),
        );
        let mut second_commit = Box::pin(second_commit);
        // Queued while the first mutation still owns the line.
        assert!(futures::poll!(second_commit.as_mut()).is_pending());
        first_mutation.end(ctx()).await.unwrap();
        second_commit.await.unwrap();

        assert_eq!(tip_of(&forked, "main").await.as_deref(), Some(ROOT_ID));
        assert_eq!(ancestry_ids(&forked).await, vec![ROOT_ID.to_string()]);
        assert_eq!(
            forked.get_name(ctx()).await.unwrap().as_deref(),
            Some("first name")
        );
        assert_eq!(
            forked.get_label(ROOT_ID, ctx()).await.unwrap().as_deref(),
            Some("first label")
        );
        source.close(ctx()).await.unwrap();
        forked.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    // === MemorySessionRepo conformance: streaming fork (WP08) ===============

    fn fork_tree(id: &str) -> ForkOptions {
        ForkOptions::Tree {
            id: Some(id.to_string()),
        }
    }

    fn fork_branch(branch: &str, entry_id: Option<String>, id: Option<&str>) -> ForkOptions {
        ForkOptions::Branch {
            branch: branch.to_string(),
            entry_id,
            position: None,
            id: id.map(str::to_string),
        }
    }

    async fn list_values(
        session: &MemorySessionFacade,
        address: &crate::agent_core::harness::session::values::ValueAddress,
    ) -> Vec<serde_json::Value> {
        session
            .read_list(address, None, ctx())
            .await
            .unwrap()
            .into_iter()
            .map(|element| element.value)
            .collect()
    }

    /// "tree fork copies lists at distinct addresses" (open and closed source).
    async fn tree_fork_copies_lists_at_distinct_addresses(closed_source: bool) {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        let events = list("test.application.events", "");
        let sibling = list("test.application.events", "other");
        let other_namespace = list("pi2.events", "");
        let absent = list("test.application.events", "absent");
        source
            .append_list(events.clone(), serde_json::json!("event"), ctx())
            .await
            .unwrap();
        source
            .append_list(sibling.clone(), serde_json::json!("sibling"), ctx())
            .await
            .unwrap();
        source
            .append_list(
                other_namespace.clone(),
                serde_json::json!("other namespace"),
                ctx(),
            )
            .await
            .unwrap();

        if closed_source {
            source.close(ctx()).await.unwrap();
        }
        let fork = repo
            .fork(source.metadata(), &fork_tree("fork"), ctx())
            .await
            .unwrap();

        assert_eq!(
            list_values(&fork, &events).await,
            vec![serde_json::json!("event")]
        );
        assert_eq!(
            list_values(&fork, &sibling).await,
            vec![serde_json::json!("sibling")]
        );
        assert_eq!(
            list_values(&fork, &other_namespace).await,
            vec![serde_json::json!("other namespace")]
        );
        assert!(list_values(&fork, &absent).await.is_empty());

        let _ = source.close(ctx()).await;
        fork.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    #[tokio::test]
    async fn replay_open_source_tree_fork_copies_lists_at_distinct_addresses() {
        tree_fork_copies_lists_at_distinct_addresses(false).await;
    }

    #[tokio::test]
    async fn replay_closed_source_tree_fork_copies_lists_at_distinct_addresses() {
        tree_fork_copies_lists_at_distinct_addresses(true).await;
    }

    /// "tree fork copies only survivors after list deletion and reappend"
    /// (open and closed source).
    async fn tree_fork_copies_only_survivors_after_delete_reappend(closed_source: bool) {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        let events = list("test.application.events", "");
        let deleted = list("test.application.events", "deleted");
        source
            .append_list(events.clone(), serde_json::json!("old"), ctx())
            .await
            .unwrap();
        source
            .append_list(deleted.clone(), serde_json::json!("removed"), ctx())
            .await
            .unwrap();
        source
            .mutate(
                |mutator, context| {
                    let events = events.clone();
                    let deleted = deleted.clone();
                    Box::pin(async move {
                        mutator
                            .commit(
                                vec![
                                    delete_list(&events),
                                    append_list(&events, serde_json::json!("temporary")),
                                    delete_list(&events),
                                    append_list(&events, serde_json::json!("first survivor")),
                                    delete_list(&deleted),
                                ],
                                context,
                            )
                            .await
                            .map(|_| ())
                    })
                },
                ctx(),
            )
            .await
            .unwrap();
        source
            .append_list(events.clone(), serde_json::json!("second survivor"), ctx())
            .await
            .unwrap();

        if closed_source {
            source.close(ctx()).await.unwrap();
        }
        let fork = repo
            .fork(source.metadata(), &fork_tree("fork"), ctx())
            .await
            .unwrap();

        assert_eq!(
            list_values(&fork, &events).await,
            vec![
                serde_json::json!("first survivor"),
                serde_json::json!("second survivor")
            ]
        );
        assert!(list_values(&fork, &deleted).await.is_empty());

        let _ = source.close(ctx()).await;
        fork.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    #[tokio::test]
    async fn replay_open_source_tree_fork_copies_only_survivors_after_delete_reappend() {
        tree_fork_copies_only_survivors_after_delete_reappend(false).await;
    }

    #[tokio::test]
    async fn replay_closed_source_tree_fork_copies_only_survivors_after_delete_reappend() {
        tree_fork_copies_only_survivors_after_delete_reappend(true).await;
    }

    /// "tree fork preserves list element sequences including gaps" (open and
    /// closed source).
    async fn tree_fork_preserves_element_sequences_with_gaps(closed_source: bool) {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        let events = list("test.application.events", "");
        let committed = source
            .mutate(
                |mutator, context| {
                    let events = events.clone();
                    Box::pin(async move {
                        mutator
                            .commit(
                                vec![
                                    set_value(&session_name(), serde_json::json!("before")),
                                    append_list(&events, serde_json::json!("first")),
                                    set_value(&session_name(), serde_json::json!("between")),
                                    append_list(&events, serde_json::json!("second")),
                                ],
                                context,
                            )
                            .await
                    })
                },
                ctx(),
            )
            .await
            .unwrap();

        if closed_source {
            source.close(ctx()).await.unwrap();
        }
        let fork = repo
            .fork(source.metadata(), &fork_tree("fork"), ctx())
            .await
            .unwrap();

        let elements = fork.read_list(&events, None, ctx()).await.unwrap();
        let first_value = serde_json::json!("first");
        let second_value = serde_json::json!("second");
        assert_eq!(
            elements
                .iter()
                .map(|element| (element.seq, &element.value))
                .collect::<Vec<_>>(),
            vec![
                (committed.seqs[1], &first_value),
                (committed.seqs[3], &second_value),
            ]
        );

        let _ = source.close(ctx()).await;
        fork.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    #[tokio::test]
    async fn replay_open_source_tree_fork_preserves_element_sequences_including_gaps() {
        tree_fork_preserves_element_sequences_with_gaps(false).await;
    }

    #[tokio::test]
    async fn replay_closed_source_tree_fork_preserves_element_sequences_including_gaps() {
        tree_fork_preserves_element_sequences_with_gaps(true).await;
    }

    /// "tree fork continues {asc,desc} pagination using source cursors"
    /// (open and closed source).
    async fn tree_fork_continues_pagination(order: AscDescOrder, closed_source: bool) {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        let events = list("test.application.events", "");
        for item in ["first", "second", "third"] {
            source
                .append_list(events.clone(), serde_json::json!(item), ctx())
                .await
                .unwrap();
        }
        let first_page = source
            .read_list(
                &events,
                Some(&ListReadOptions {
                    order: Some(order),
                    limit: Some(2),
                    cursor: None,
                }),
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(first_page.len(), 2);
        let cursor = ListCursor {
            seq: first_page[1].seq,
        };
        let last_page = source
            .read_list(
                &events,
                Some(&ListReadOptions {
                    order: Some(order),
                    cursor: Some(cursor),
                    limit: Some(2),
                }),
                ctx(),
            )
            .await
            .unwrap();
        let expected_after_cursor = serde_json::json!(if order == AscDescOrder::Asc {
            "third"
        } else {
            "first"
        });
        assert_eq!(
            last_page
                .iter()
                .map(|e| e.value.clone())
                .collect::<Vec<_>>(),
            vec![expected_after_cursor]
        );
        let end_cursor = ListCursor {
            seq: last_page[0].seq,
        };

        if closed_source {
            source.close(ctx()).await.unwrap();
        }
        let fork = repo
            .fork(source.metadata(), &fork_tree("fork"), ctx())
            .await
            .unwrap();

        assert_eq!(
            fork.read_list(
                &events,
                Some(&ListReadOptions {
                    order: Some(order),
                    limit: Some(2),
                    cursor: None,
                }),
                ctx()
            )
            .await
            .unwrap(),
            first_page
        );
        assert_eq!(
            fork.read_list(
                &events,
                Some(&ListReadOptions {
                    order: Some(order),
                    cursor: Some(cursor),
                    limit: Some(2),
                }),
                ctx()
            )
            .await
            .unwrap(),
            last_page
        );
        assert!(fork
            .read_list(
                &events,
                Some(&ListReadOptions {
                    order: Some(order),
                    cursor: Some(end_cursor),
                    limit: Some(2),
                }),
                ctx()
            )
            .await
            .unwrap()
            .is_empty());

        let _ = source.close(ctx()).await;
        fork.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    #[tokio::test]
    async fn replay_open_source_tree_fork_continues_asc_pagination_using_source_cursors() {
        tree_fork_continues_pagination(AscDescOrder::Asc, false).await;
    }

    #[tokio::test]
    async fn replay_open_source_tree_fork_continues_desc_pagination_using_source_cursors() {
        tree_fork_continues_pagination(AscDescOrder::Desc, false).await;
    }

    #[tokio::test]
    async fn replay_closed_source_tree_fork_continues_asc_pagination_using_source_cursors() {
        tree_fork_continues_pagination(AscDescOrder::Asc, true).await;
    }

    #[tokio::test]
    async fn replay_closed_source_tree_fork_continues_desc_pagination_using_source_cursors() {
        tree_fork_continues_pagination(AscDescOrder::Desc, true).await;
    }

    /// "excludes overwritten and unchanged application values" (open and
    /// closed source).
    async fn branch_fork_excludes_overwritten_and_unchanged_values(closed_source: bool) {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        let state = value("test.application.state", "");
        let unchanged = value("test.application.settings", "");
        let branch = source.create_branch("review", None, ctx()).await.unwrap();
        source
            .mutate(
                |mutator, context| {
                    let state = state.clone();
                    let unchanged = unchanged.clone();
                    Box::pin(async move {
                        mutator
                            .commit(
                                vec![
                                    set_value(&lane_config("review"), configuration()),
                                    set_value(&lane_state("review"), idle_lane_state()),
                                    set_value(&state, serde_json::json!("v1")),
                                    set_value(&unchanged, serde_json::json!("predates fork point")),
                                ],
                                context,
                            )
                            .await
                            .map(|_| ())
                    })
                },
                ctx(),
            )
            .await
            .unwrap();
        let entry_id = branch
            .append_custom_entry("fork-point".to_string(), None, ctx())
            .await
            .unwrap();
        source
            .set_value(state.clone(), serde_json::json!("v2"), ctx())
            .await
            .unwrap();
        assert_eq!(
            source
                .get_value(&state, ctx())
                .await
                .unwrap()
                .unwrap()
                .value,
            serde_json::json!("v2")
        );

        if closed_source {
            source.close(ctx()).await.unwrap();
        }
        let fork = repo
            .fork(
                source.metadata(),
                &fork_branch("review", Some(entry_id.clone()), None),
                ctx(),
            )
            .await
            .unwrap();

        assert_eq!(
            tip_of(&fork, "review").await.as_deref(),
            Some(entry_id.as_str())
        );
        assert!(fork.get_value(&state, ctx()).await.unwrap().is_none());
        assert!(fork.get_value(&unchanged, ctx()).await.unwrap().is_none());

        let _ = source.close(ctx()).await;
        fork.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    #[tokio::test]
    async fn replay_open_source_branch_fork_excludes_overwritten_and_unchanged_values() {
        branch_fork_excludes_overwritten_and_unchanged_values(false).await;
    }

    #[tokio::test]
    async fn replay_closed_source_branch_fork_excludes_overwritten_and_unchanged_values() {
        branch_fork_excludes_overwritten_and_unchanged_values(true).await;
    }

    /// "excludes deleted/reappended and untouched application lists" (open
    /// and closed source).
    async fn branch_fork_excludes_deleted_reappended_and_untouched_lists(closed_source: bool) {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        let events = list("test.application.events", "");
        let untouched = list("test.application.events", "untouched");
        let branch = source.create_branch("review", None, ctx()).await.unwrap();
        source
            .mutate(
                |mutator, context| {
                    let events = events.clone();
                    let untouched = untouched.clone();
                    Box::pin(async move {
                        mutator
                            .commit(
                                vec![
                                    set_value(&lane_config("review"), configuration()),
                                    set_value(&lane_state("review"), idle_lane_state()),
                                    append_list(&events, serde_json::json!("old first")),
                                    append_list(&events, serde_json::json!("old second")),
                                    append_list(
                                        &untouched,
                                        serde_json::json!("predates fork point"),
                                    ),
                                ],
                                context,
                            )
                            .await
                            .map(|_| ())
                    })
                },
                ctx(),
            )
            .await
            .unwrap();
        let entry_id = branch
            .append_custom_entry("fork-point".to_string(), None, ctx())
            .await
            .unwrap();
        source.delete_list(events.clone(), ctx()).await.unwrap();
        source
            .append_list(events.clone(), serde_json::json!("new"), ctx())
            .await
            .unwrap();
        assert_eq!(
            list_values(&source, &events).await,
            vec![serde_json::json!("new")]
        );

        if closed_source {
            source.close(ctx()).await.unwrap();
        }
        let fork = repo
            .fork(
                source.metadata(),
                &fork_branch("review", Some(entry_id.clone()), None),
                ctx(),
            )
            .await
            .unwrap();

        assert_eq!(
            tip_of(&fork, "review").await.as_deref(),
            Some(entry_id.as_str())
        );
        assert!(fork
            .read_list(&events, None, ctx())
            .await
            .unwrap()
            .is_empty());
        assert!(fork
            .read_list(&untouched, None, ctx())
            .await
            .unwrap()
            .is_empty());

        let _ = source.close(ctx()).await;
        fork.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }

    #[tokio::test]
    async fn replay_open_source_branch_fork_excludes_deleted_reappended_and_untouched_lists() {
        branch_fork_excludes_deleted_reappended_and_untouched_lists(false).await;
    }

    #[tokio::test]
    async fn replay_closed_source_branch_fork_excludes_deleted_reappended_and_untouched_lists() {
        branch_fork_excludes_deleted_reappended_and_untouched_lists(true).await;
    }

    /// "ignores malformed unrelated lanes" (fork lane validation).
    #[tokio::test]
    async fn replay_ignores_malformed_unrelated_lanes() {
        let repo = fixed_repo();
        let source = create(&repo, "source").await;
        source.create_branch("main", None, ctx()).await.unwrap();
        commit_writes(
            &source,
            vec![
                set_value(&lane_config("main"), configuration()),
                set_value(&lane_state("main"), idle_lane_state()),
                set_value(&lane_config("unrelated"), configuration()),
            ],
        )
        .await;

        let tree = repo
            .fork(source.metadata(), &fork_tree("tree"), ctx())
            .await
            .unwrap();
        let branch = repo
            .fork(
                source.metadata(),
                &fork_branch("main", None, Some("branch")),
                ctx(),
            )
            .await
            .unwrap();
        assert_eq!(tip_of(&branch, "main").await, None);
        source.close(ctx()).await.unwrap();
        tree.close(ctx()).await.unwrap();
        branch.close(ctx()).await.unwrap();
        repo.close(ctx()).await.unwrap();
    }
}
