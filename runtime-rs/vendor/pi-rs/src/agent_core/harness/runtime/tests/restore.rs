use super::super::durable::*;
use super::super::restore::*;
use crate::agent_core::harness::session::{
    self as session, InstrumentedStorage, MemoryStorage, MemoryStorageOptions, Session,
    SessionMetadata, StorageBackedSession, Write,
};
use crate::agent_core::harness::{background_context, DEFAULT_COMPACTION_SETTINGS};
use serde_json::{json, Value};
use std::sync::Arc;

fn configuration() -> Value {
    json!({"model":{"provider":"test","modelId":"model"},"thinkingLevel":"off","activeToolNames":[]})
}
fn scope() -> Value {
    json!({"control":{"status":"running"},"settings":{"compaction":DEFAULT_COMPACTION_SETTINGS,"steeringMode":"all","followUpMode":"all","toolExecution":"parallel"},"latestAssistantEntryId":null})
}
pub(super) fn state(phase: Value) -> Value {
    let mut value = scope();
    value
        .as_object_mut()
        .unwrap()
        .extend(phase.as_object().unwrap().clone());
    value
}
fn checkpoint() -> Value {
    json!({"at":"checkpoint","continuation":{"kind":"need_assistant","overflowRecoveryUsed":false},"triggerEntryId":"missing-trigger"})
}
fn metadata(intent: Value) -> Value {
    json!({"operationId":"op","lane":"main","sourceTipId":null,"startedAt":1,"intent":intent})
}
fn projection(current: Option<&str>) -> Value {
    json!({"currentOperationId":current,"lastOperationId":null,"inbox":[]})
}
pub(super) fn fixture() -> (Arc<StorageBackedSession>, Arc<InstrumentedStorage>) {
    let memory = Arc::new(MemoryStorage::new(MemoryStorageOptions {
        now: Some(Arc::new(|| 42)),
    }));
    let storage = InstrumentedStorage::new(memory);
    (
        Arc::new(StorageBackedSession::new(
            SessionMetadata {
                id: "test".into(),
                created_at: 42,
                storage_version: 1,
                ..Default::default()
            },
            storage.clone(),
        )),
        storage,
    )
}
pub(super) async fn commit(session: &StorageBackedSession, writes: Vec<Write>) {
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
async fn configure(session: &StorageBackedSession, lane: &str) {
    commit(
        session,
        vec![
            session::set_value(&session::branch_tip(lane), Value::Null),
            session::set_value(&session::lane_config(lane), configuration()),
            session::set_value(&session::lane_state(lane), projection(None)),
        ],
    )
    .await;
}
async fn install(session: &StorageBackedSession, meta: Value, state: Value) {
    commit(
        session,
        vec![
            session::set_value(&session::operation_meta("op"), meta),
            session::set_value(&session::operation_state("op"), state),
            session::set_value(&session::lane_state("main"), projection(Some("op"))),
        ],
    )
    .await;
}
pub(super) fn phases() -> Vec<Value> {
    let retry = json!({"maxAttempts":4,"baseDelayMs":1000,"maxAgentDelayMs":60000});
    let generation = json!({"stepId":"step","triggerEntryId":"trigger","configuration":configuration(),"streamOptions":{},"retryPolicy":retry,"overflowRecoveryUsed":false});
    let task = json!({"taskId":"summary","boundary":{"kind":"finish"}});
    let summary = json!({"resultEntryId":"result","configuration":configuration(),"streamOptions":{},"retryPolicy":retry});
    vec![
        json!({"at":"starting"}),
        checkpoint(),
        json!({"at":"assistant.ready","generationContext":generation,"nextAttempt":1}),
        json!({"at":"assistant.effect_pending","generationContext":generation,"attempt":1,"responseEntryId":"response","usageId":"usage","intendedOutputLimit":4096,"contextWindow":128000}),
        json!({"at":"assistant.retry_wait","generationContext":generation,"nextAttempt":2,"notBefore":5000,"errorMessage":"retry"}),
        json!({"at":"tools","batch":{"assistantEntryId":"assistant","configuration":configuration(),"turnId":"turn","calls":[{"sourceIndex":3,"resultEntryId":"r3","status":"planned"},{"sourceIndex":5,"resultEntryId":"r5","status":"effect_pending","replay":"safe"},{"sourceIndex":6,"resultEntryId":"r6","status":"outcome_ready","terminate":false},{"sourceIndex":8,"resultEntryId":"r8","status":"completed","terminate":true}]}}),
        json!({"at":"deferred.suspended","stepId":"step","sourceEntryId":"source","poll":1,"configuration":configuration(),"streamOptions":{"deferred":true}}),
        json!({"at":"deferred.effect_pending","stepId":"step","sourceEntryId":"source","poll":2,"configuration":configuration(),"streamOptions":{},"responseEntryId":"response","usageId":"usage"}),
        json!({"at":"summary.deciding","task":task}),
        json!({"at":"summary.ready","task":task,"summaryContext":summary,"nextAttempt":1}),
        json!({"at":"summary.effect_pending","task":task,"summaryContext":summary,"attempt":1,"request":{"index":2,"usageId":"u2"},"usageIds":["u1","u2"]}),
        json!({"at":"summary.retry_wait","task":task,"summaryContext":summary,"nextAttempt":2,"notBefore":5000,"errorMessage":"retry"}),
        json!({"at":"navigation.ready_to_commit","targetId":null}),
    ]
}

#[test]
fn all_thirteen_durable_leaves_round_trip_the_flat_wire_shape() {
    let variants = phases();
    assert_eq!(variants.len(), 13);
    for phase in variants {
        let raw = state(phase);
        let parsed: OperationState = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(&parsed).unwrap(), raw);
        assert_eq!(parsed.at(), raw["at"]);
        assert_eq!(
            serde_json::to_value(parsed.operation_scope()).unwrap(),
            scope()
        );
    }
    let mut cancelled = state(checkpoint());
    cancelled["control"] = json!({"status":"cancel_requested","requestedAt":42});
    let parsed: OperationState = serde_json::from_value(cancelled.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), cancelled);
}
#[test]
fn intent_reachability_matrix_checks_summary_boundaries_labels_and_instructions() {
    let run: OperationIntent =
        serde_json::from_value(json!({"kind":"run","promptEntryIds":[]})).unwrap();
    let compact: OperationIntent = serde_json::from_value(json!({"kind":"compaction"})).unwrap();
    for phase in phases() {
        let parsed: OperationState = serde_json::from_value(state(phase)).unwrap();
        let summary = parsed.at().starts_with("summary.");
        assert_eq!(
            state_matches_intent(&run, &parsed),
            !summary && parsed.at() != "navigation.ready_to_commit"
        );
        assert_eq!(state_matches_intent(&compact, &parsed), summary);
    }
    for summary_phase in [
        "summary.deciding",
        "summary.ready",
        "summary.effect_pending",
        "summary.retry_wait",
    ] {
        for boundary in [
            json!({"kind":"finish"}),
            json!({"kind":"resume_checkpoint","resumeAfter":{"continuation":{"kind":"may_finish","includeFinalAssistant":true},"triggerEntryId":"trigger"}}),
            json!({"kind":"commit_navigation","targetId":"target","label":"label"}),
        ] {
            let mut phase = phases()
                .into_iter()
                .find(|v| v["at"] == summary_phase)
                .unwrap();
            phase["task"]["boundary"] = boundary.clone();
            phase["task"]["customInstructions"] = json!("instructions");
            let parsed = serde_json::from_value(state(phase)).unwrap();
            assert_eq!(
                state_matches_intent(&run, &parsed),
                boundary["kind"] == "resume_checkpoint"
            );
            assert_eq!(
                state_matches_intent(&compact, &parsed),
                boundary["kind"] == "finish"
            );
            for target in ["target", "wrong"] {
                for label in [Some("label"), None, Some("wrong")] {
                    for instructions in [Some("instructions"), None, Some("wrong")] {
                        for summarize in [true, false] {
                            let intent = OperationIntent::Navigation {
                                target_id: Some(target.into()),
                                summarize,
                                label: label.map(str::to_owned),
                                custom_instructions: instructions.map(str::to_owned),
                            };
                            assert_eq!(
                                state_matches_intent(&intent, &parsed),
                                summarize
                                    && boundary["kind"] == "commit_navigation"
                                    && target == "target"
                                    && label == Some("label")
                                    && instructions == Some("instructions")
                            );
                        }
                    }
                }
            }
        }
    }
    for target in [None, Some("target")] {
        for label in [None, Some("label")] {
            let parsed = serde_json::from_value(state(
                json!({"at":"navigation.ready_to_commit","targetId":target,"label":label}),
            ))
            .unwrap();
            let exact = OperationIntent::Navigation {
                target_id: target.map(str::to_owned),
                summarize: false,
                label: label.map(str::to_owned),
                custom_instructions: Some("ignored without summary".into()),
            };
            assert!(state_matches_intent(&exact, &parsed));
        }
    }
}
#[tokio::test]
async fn restore_idle_keeps_last_result_id_without_fetching_the_result_or_writing() {
    let (session, storage) = fixture();
    configure(&session, "main").await;
    let mut idle = projection(None);
    idle["lastOperationId"] = json!("missing-result");
    idle["inbox"] = json!([{"entryId":"missing-pending","kind":"followUp"}]);
    commit(
        &session,
        vec![session::set_value(&session::lane_state("main"), idle)],
    )
    .await;
    storage.clear_commit_attempts();
    let restored = restore_lane(session.as_ref(), "main", background_context())
        .await
        .unwrap();
    assert!(restored.operation.is_none());
    assert_eq!(
        restored.last_operation_id.as_deref(),
        Some("missing-result")
    );
    assert_eq!(restored.inbox[0].entry_id, "missing-pending");
    assert!(storage.get_commit_attempts().is_empty());
}
#[tokio::test]
async fn restore_open_operation_does_not_read_referenced_payloads() {
    let (session, storage) = fixture();
    configure(&session, "main").await;
    let meta = metadata(json!({"kind":"run","promptEntryIds":["missing-prompt"]}));
    let raw = state(checkpoint());
    install(&session, meta.clone(), raw.clone()).await;
    storage.clear_commit_attempts();
    let restored = restore_lane(session.as_ref(), "main", background_context())
        .await
        .unwrap();
    let operation = restored.operation.unwrap();
    assert_eq!(serde_json::to_value(operation.meta).unwrap(), meta);
    assert_eq!(serde_json::to_value(operation.state).unwrap(), raw);
    assert!(storage.get_commit_attempts().is_empty());
}
#[tokio::test]
async fn restore_rejects_identity_foreign_lane_and_intent_mismatch_with_exact_errors() {
    for (field, value, expected) in [
        (
            "operationId",
            json!("other"),
            "Operation op metadata names operation \"other\"",
        ),
        (
            "lane",
            json!("worker"),
            "Operation op belongs to lane \"worker\", not \"main\"",
        ),
        (
            "intent",
            json!({"kind":"navigation","targetId":null,"summarize":false}),
            "Operation op intent navigation does not match state checkpoint",
        ),
    ] {
        let (session, _) = fixture();
        configure(&session, "main").await;
        let mut meta = metadata(json!({"kind":"run","promptEntryIds":[]}));
        meta[field] = value;
        install(&session, meta, state(checkpoint())).await;
        let error = restore_lane(session.as_ref(), "main", background_context())
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), expected);
        assert!(error
            .downcast_ref::<session::SessionInvariantError>()
            .is_some());
    }
}
#[tokio::test]
async fn lane_presence_classification_and_error_precedence_cover_all_eight_combinations() {
    for bits in 0..8 {
        let (session, _) = fixture();
        let mut writes = Vec::new();
        if bits & 1 != 0 {
            writes.push(session::set_value(
                &session::branch_tip("main"),
                Value::Null,
            ));
        }
        if bits & 2 != 0 {
            writes.push(session::set_value(
                &session::lane_config("main"),
                configuration(),
            ));
        }
        if bits & 4 != 0 {
            writes.push(session::set_value(
                &session::lane_state("main"),
                projection(None),
            ));
        }
        if !writes.is_empty() {
            commit(&session, writes).await;
        }
        let result = restore_lane(session.as_ref(), "main", background_context()).await;
        if bits == 7 {
            assert!(result.is_ok());
        } else {
            let missing = if bits & 1 == 0 {
                "branch.tip"
            } else if bits & 2 == 0 {
                "lane.config"
            } else {
                "lane.state"
            };
            assert_eq!(
                result.unwrap_err().to_string(),
                format!("Lane \"main\" is missing {missing}")
            );
        }
        let all = restore_session(session.as_ref(), background_context()).await;
        if bits == 0 || bits == 1 {
            assert!(all.unwrap().is_empty());
        } else if bits == 7 {
            assert_eq!(all.unwrap().len(), 1);
        } else {
            assert!(all.is_err());
        }
    }
}
#[tokio::test]
async fn current_operation_requires_meta_before_state_and_preserves_null_tip() {
    for bits in 0..3 {
        let (session, _) = fixture();
        configure(&session, "main").await;
        let mut writes = vec![session::set_value(
            &session::lane_state("main"),
            projection(Some("op")),
        )];
        if bits & 1 != 0 {
            writes.push(session::set_value(
                &session::operation_meta("op"),
                metadata(json!({"kind":"run","promptEntryIds":[]})),
            ));
        }
        if bits & 2 != 0 {
            writes.push(session::set_value(
                &session::operation_state("op"),
                state(checkpoint()),
            ));
        }
        commit(&session, writes).await;
        let missing = if bits & 1 == 0 { "meta" } else { "state" };
        assert_eq!(
            restore_lane(session.as_ref(), "main", background_context())
                .await
                .unwrap_err()
                .to_string(),
            format!("Operation op is missing op.{missing}")
        );
    }
}
#[tokio::test]
async fn restores_every_configured_lane_once_skips_plain_branches_and_never_commits() {
    let (session, storage) = fixture();
    configure(&session, "z").await;
    configure(&session, "a").await;
    commit(
        &session,
        vec![session::set_value(
            &session::branch_tip("plain"),
            Value::Null,
        )],
    )
    .await;
    storage.clear_commit_attempts();
    let before = session
        .scan_values(&session::lane_state(""), background_context())
        .await
        .unwrap();
    let lanes = restore_session(session.as_ref(), background_context())
        .await
        .unwrap();
    assert_eq!(lanes.len(), 2);
    let names: std::collections::HashSet<_> = lanes.iter().map(|v| v.0.as_str()).collect();
    assert_eq!(names, std::collections::HashSet::from(["z", "a"]));
    assert!(storage.get_commit_attempts().is_empty());
    assert_eq!(
        session
            .scan_values(&session::lane_state(""), background_context())
            .await
            .unwrap(),
        before
    );
}
#[tokio::test]
async fn restoration_waits_for_one_coherent_mutation_barrier() {
    let (session, _) = fixture();
    configure(&session, "main").await;
    let handle = session.begin_mutation(background_context()).await.unwrap();
    let cloned = session.clone();
    let mut restoring =
        tokio::spawn(async move { restore_session(cloned.as_ref(), background_context()).await });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut restoring)
            .await
            .is_err()
    );
    let mut config = configuration();
    config["model"]["modelId"] = json!("updated");
    handle
        .commit(
            vec![
                session::set_value(&session::lane_config("main"), config),
                session::set_value(
                    &session::lane_state("main"),
                    json!({"currentOperationId":null,"lastOperationId":"paired","inbox":[]}),
                ),
            ],
            background_context(),
        )
        .await
        .unwrap();
    handle.end(background_context()).await.unwrap();
    let lanes = tokio::time::timeout(std::time::Duration::from_secs(2), restoring)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(lanes[0].1.configuration.model.model_id, "updated");
    assert_eq!(lanes[0].1.last_operation_id.as_deref(), Some("paired"));
}
