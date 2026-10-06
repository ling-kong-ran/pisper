use super::super::progress::read_assistant_frames;
use super::super::transcript::*;
use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::session::*;
use crate::agent_core::types::AgentMessage;
use crate::ai::types::Usage;
use serde_json::{json, Value};
use std::sync::Arc;

fn user(text: &str) -> AgentMessage {
    serde_json::from_value(json!({"role":"user","content":text,"timestamp":1})).unwrap()
}
fn staged(id: &str) -> NewEntry {
    NewEntry::Message {
        id: id.into(),
        parent_id: Some("old-parent".into()),
        message: user(id),
        terminate: None,
    }
}
fn fixture() -> (Arc<StorageBackedSession>, Arc<InstrumentedStorage>) {
    let storage = InstrumentedStorage::new(Arc::new(MemoryStorage::new(
        MemoryStorageOptions::default(),
    )));
    (
        Arc::new(StorageBackedSession::new(
            SessionMetadata {
                id: "test".into(),
                created_at: 0,
                storage_version: 1,
                ..Default::default()
            },
            storage.clone(),
        )),
        storage,
    )
}
async fn write(session: &StorageBackedSession, writes: Vec<Write>) {
    session
        .mutate(
            move |reader, context| {
                Box::pin(async move { reader.commit(writes, context).await.map(|_| ()) })
            },
            background_context(),
        )
        .await
        .unwrap();
}
#[test]
fn chains_all_staged_entry_kinds_without_changing_inputs() {
    let items: Vec<NewEntry> = vec![
        staged("message"),
        serde_json::from_value(json!({"type":"custom","id":"custom","parentId":null,"customType":"test","data":null})).unwrap(),
        serde_json::from_value(json!({"type":"compaction","id":"compaction","parentId":null,"summary":"s","retainedTail":[],"tokensBefore":3,"fromHook":true})).unwrap(),
        serde_json::from_value(json!({"type":"branch_summary","id":"branch","parentId":null,"fromId":"other","summary":"s","fromHook":false})).unwrap(),
    ];
    let before = items.clone();
    for initial in [None, Some("root")] {
        let chained = chain_entries(initial, &items);
        let mut parent = initial;
        for (item, input) in chained.iter().zip(&items) {
            let mut expected = serde_json::to_value(input).unwrap();
            expected["parentId"] = json!(parent);
            assert_eq!(serde_json::to_value(item).unwrap(), expected);
            parent = Some(input.id());
        }
    }
    assert_eq!(items, before);
    assert!(chain_entries(Some("root"), &[]).is_empty());
}
#[test]
fn lifecycle_envelopes_keep_exact_order_and_do_not_put_run_id_on_entry_added() {
    let entry = staged("message").into_entry(7, 9);
    for run_id in [None, Some("run")] {
        let events = serde_json::to_value(entry_lifecycle_events(&entry, "lane", run_id)).unwrap();
        let mut start = json!({"type":"message_start","lane":"lane","message":user("message")});
        let mut end = json!({"type":"message_end","lane":"lane","message":user("message"),"entryId":"message"});
        if let Some(run_id) = run_id {
            start["runId"] = json!(run_id);
            end["runId"] = json!(run_id);
        }
        assert_eq!(
            events,
            json!([start, end, {"type":"entry_added","lane":"lane","entry":entry}])
        );
        let custom: Entry = serde_json::from_value(json!({"type":"custom","id":"custom","parentId":null,"seq":8,"timestamp":9,"customType":"test","data":null})).unwrap();
        assert_eq!(
            serde_json::to_value(entry_lifecycle_events(&custom, "lane", run_id)).unwrap(),
            json!([{"type":"entry_added","lane":"lane","entry":custom}])
        );
    }
}
#[test]
fn committed_events_use_write_offsets_and_actual_noncontiguous_sequences() {
    let commit = CommitResult {
        first_seq: 10,
        seqs: vec![10, 15, 31, 44],
        timestamp: 123,
        stats: SessionStats {
            message_count: 0,
            usage: Usage::default(),
        },
    };
    let entries = chain_entries(Some("root"), &[staged("a"), staged("b")]);
    let events = committed_entry_events(&entries, &commit, "main", Some("run"), 1).unwrap();
    for (offset, seq) in [(2, 15), (5, 31)] {
        let EntryLifecycleEvent::EntryAdded { entry, .. } = &events[offset] else {
            panic!()
        };
        assert_eq!(entry.seq(), seq);
        assert_eq!(entry.timestamp(), 123);
    }
    assert!(committed_entry_events(&entries, &commit, "main", None, 3).is_err());
    assert!(committed_entry_events(&entries, &commit, "main", None, usize::MAX).is_err());
    assert!(committed_entry_events(&[], &commit, "main", None, 4)
        .unwrap()
        .is_empty());
}
#[tokio::test]
async fn queue_projection_preserves_kinds_order_duplicates_and_null_payloads_without_writes() {
    let (session, storage) = fixture();
    write(
        &session,
        vec![
            set_value(
                &pending_entry("m"),
                json!({"type":"message","payload":user("m")}),
            ),
            set_value(
                &pending_entry("c"),
                json!({"type":"custom","customType":"null","payload":null}),
            ),
            set_value(
                &pending_entry("a"),
                json!({"type":"custom","customType":"absent"}),
            ),
        ],
    )
    .await;
    storage.clear_commit_attempts();
    let inbox: Vec<InboxItem> = serde_json::from_value(json!([
        {"entryId":"m","kind":"steer"},{"entryId":"m","kind":"followUp"},{"entryId":"m","kind":"nextRun"},{"entryId":"m","kind":"write"},{"entryId":"c","kind":"write"},{"entryId":"a","kind":"write"}
    ])).unwrap();
    let queues = session
        .mutate(
            move |reader, context| {
                Box::pin(async move { read_lane_queues(reader, &inbox, context).await })
            },
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(queues).unwrap(),
        json!([
            {"type":"message","entryId":"m","kind":"steer","message":user("m")},
            {"type":"message","entryId":"m","kind":"followUp","message":user("m")},
            {"type":"message","entryId":"m","kind":"nextRun","message":user("m")},
            {"type":"message","entryId":"m","kind":"write","message":user("m")},
            {"type":"custom","entryId":"c","kind":"write","customType":"null","data":null},
            {"type":"custom","entryId":"a","kind":"write","customType":"absent"}
        ])
    );
    assert!(storage.get_commit_attempts().is_empty());
}
#[tokio::test]
async fn queue_projection_pins_missing_payload_and_wrong_type_invariant_errors() {
    let (session, _) = fixture();
    write(
        &session,
        vec![set_value(
            &pending_entry("custom"),
            json!({"type":"custom","customType":"test"}),
        )],
    )
    .await;
    for (kind, name) in [
        (InboxItemKind::Steer, "steer"),
        (InboxItemKind::FollowUp, "followUp"),
        (InboxItemKind::NextRun, "nextRun"),
        (InboxItemKind::Write, "write"),
    ] {
        for id in ["absent", "custom"] {
            if id == "custom" && kind == InboxItemKind::Write {
                continue;
            }
            let error = session
                .mutate(
                    move |reader, context| {
                        Box::pin(async move {
                            read_lane_queues(
                                reader,
                                &[InboxItem {
                                    entry_id: id.into(),
                                    kind,
                                }],
                                context,
                            )
                            .await
                        })
                    },
                    background_context(),
                )
                .await
                .unwrap_err();
            assert!(error.downcast_ref::<SessionInvariantError>().is_some());
            assert_eq!(
                error.to_string(),
                if id == "absent" {
                    format!("Pending {name} entry absent is missing its payload")
                } else {
                    format!("Pending {name} entry custom is not a message")
                }
            );
        }
    }
}
#[tokio::test]
async fn pending_messages_keep_caller_order_and_description_specific_failures() {
    let (session, _) = fixture();
    write(
        &session,
        vec![
            set_value(
                &pending_entry("a"),
                json!({"type":"message","payload":user("a")}),
            ),
            set_value(
                &pending_entry("b"),
                json!({"type":"message","payload":user("b")}),
            ),
            set_value(
                &pending_entry("custom"),
                json!({"type":"custom","customType":"test"}),
            ),
        ],
    )
    .await;
    let messages = session
        .mutate(
            |reader, context| {
                Box::pin(async move {
                    read_pending_messages(
                        reader,
                        &["b".into(), "a".into(), "b".into()],
                        "Prompt",
                        context,
                    )
                    .await
                })
            },
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(messages).unwrap(),
        json!([{"entryId":"b","message":user("b")},{"entryId":"a","message":user("a")},{"entryId":"b","message":user("b")}])
    );
    for id in ["absent", "custom"] {
        let error = session
            .mutate(
                move |reader, context| {
                    Box::pin(async move {
                        read_pending_messages(reader, &[id.into()], "Prompt", context).await
                    })
                },
                background_context(),
            )
            .await
            .unwrap_err();
        assert!(error.downcast_ref::<SessionInvariantError>().is_some());
        assert_eq!(
            error.to_string(),
            format!("Prompt {id} is missing its message payload")
        );
    }
    assert!(session
        .mutate(
            |reader, context| Box::pin(async move {
                read_pending_messages(reader, &[], "Prompt", context).await
            }),
            background_context()
        )
        .await
        .unwrap()
        .is_empty());
}
#[tokio::test]
async fn paged_progress_recovers_raw_frames_across_exact_thousand_boundaries_and_sequence_gaps() {
    for count in [0usize, 1, 999, 1000, 1001, 2000, 2003] {
        let (session, storage) = fixture();
        let address = pending_assistant_frames("operation", "response");
        let expected: Vec<Value> = (0..count)
            .map(|index| {
                if index % 17 == 0 {
                    Value::Null
                } else {
                    json!({"type":"text_delta","index":index,"unknown":null})
                }
            })
            .collect();
        let writes = expected
            .iter()
            .flat_map(|frame| {
                [
                    append_list(&address, frame.clone()),
                    set_value(&session_name(), json!("sequence gap")),
                ]
            })
            .collect();
        write(&session, writes).await;
        write(
            &session,
            vec![append_list(
                &pending_assistant_frames("foreign", "response"),
                json!("foreign"),
            )],
        )
        .await;
        storage.clear_commit_attempts();
        let frames = session
            .mutate(
                |reader, context| {
                    Box::pin(async move {
                        read_assistant_frames(reader, "operation", "response", context).await
                    })
                },
                background_context(),
            )
            .await
            .unwrap();
        assert_eq!(frames, expected, "count {count}");
        assert!(storage.get_commit_attempts().is_empty());
    }
}

// --- bounded context readers (transcript.ts:50-84, real Lane) ---
#[allow(dead_code)]
mod bounded_readers_tests {
    use super::*;
    use crate::agent_core::harness::runtime::drive_pass::{Drive, DriveOptions};
    use crate::agent_core::harness::runtime::durable::{OperationPhase, OperationState};
    use crate::agent_core::harness::runtime::lane::{
        ContinueOperationResult, EmitBatch, Lane as LaneT, OperationRequest, RuntimeConfig,
    };
    use crate::agent_core::harness::runtime::restore::restore_lane;
    use crate::agent_core::harness::session::{
        LaneConfiguration, LaneModel, MemoryStorage, SessionMetadata, StorageBackedSession,
    };
    use crate::agent_core::harness::{background_context, DEFAULT_COMPACTION_SETTINGS};
    use crate::agent_core::types::{QueueMode, ThinkingLevel, ToolExecutionMode};
    use crate::ai::models::faux::{faux_provider, FauxProviderOptions};
    use crate::ai::models::{create_models, CreateModelsOptions};

    async fn create_lane() -> Arc<LaneT> {
        let storage = MemoryStorage::new(MemoryStorageOptions::default());
        let sess = Arc::new(StorageBackedSession::new(
            SessionMetadata {
                id: "transcript-bounded-test".into(),
                created_at: 1,
                storage_version: 1,
                ..Default::default()
            },
            Arc::new(storage),
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
            set_value(&branch_tip("main"), serde_json::Value::Null),
            set_value(
                &lane_config("main"),
                lane_configuration_value(&configuration),
            ),
            set_value(
                &lane_state("main"),
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
        LaneT::new(
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

    fn capability() -> OperationState {
        OperationState {
            scope: serde_json::from_str(
                r#"{"control":{"status":"running"},"settings":{"compaction":{"enabled":true,"reserveTokens":16384,"keepRecentTokens":20000},"steeringMode":"all","followUpMode":"all","toolExecution":"parallel"},"latestAssistantEntryId":null}"#,
            )
            .unwrap(),
            phase: OperationPhase::Starting,
        }
    }

    #[tokio::test]
    async fn bounded_entries_and_context_follow_the_accepted_run() {
        let lane = create_lane().await;
        let admission = lane
            .accept(
                &OperationRequest::Prompt {
                    prompt: "hello bounded".to_string(),
                },
                background_context(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(admission.kind, "run");
        let drive = Drive::new(
            &DriveOptions {
                operation_id: admission.operation_id.clone(),
                wait_for_retry: None,
                poll_deferred: None,
            },
            background_context(),
        );
        let capability = capability();
        let entries = match read_bounded_entries(&lane, &drive, &capability)
            .await
            .unwrap()
        {
            ContinueOperationResult::Result { value } => value,
            ContinueOperationResult::CancelRequested => panic!("durable control is running"),
        };
        assert!(!entries.is_empty());
        assert!(matches!(
            entries.first().unwrap(),
            Entry::Message {
                message: AgentMessage::User(_),
                ..
            }
        ));
        let messages = match read_bounded_context(&lane, &drive, &capability)
            .await
            .unwrap()
        {
            ContinueOperationResult::Result { value } => value,
            ContinueOperationResult::CancelRequested => panic!("durable control is running"),
        };
        assert!(!messages.is_empty());
    }
}
