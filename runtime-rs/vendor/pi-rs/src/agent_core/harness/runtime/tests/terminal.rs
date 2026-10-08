use super::super::drive::terminal::*;
use super::super::durable::{OperationMeta, OperationState};
use super::restore::{commit, fixture, phases, state};
use crate::agent_core::harness::context::background_context;
use crate::agent_core::harness::session::*;
use serde_json::{json, Value};

fn leftovers(operation: &str) -> Vec<ValueAddress> {
    vec![
        operation_meta(operation),
        operation_state(operation),
        operation_tool_args(operation, "step", 0),
        operation_tool_memo(operation, "invocation", "memo"),
        operation_preparation(operation, "task"),
        pending_tool_output(operation, "invocation"),
    ]
}
#[tokio::test]
async fn cleanup_every_phase_selects_only_owned_families_and_the_exact_live_frame_list() {
    for phase in phases() {
        let phase_name = phase["at"].as_str().unwrap().to_owned();
        let is_effect =
            ["assistant.effect_pending", "deferred.effect_pending"].contains(&phase_name.as_str());
        let is_tools = phase_name == "tools";
        let operation: OperationState = serde_json::from_value(state(phase)).unwrap();
        let (session, storage) = fixture();
        let own = leftovers("op");
        let foreign = leftovers("op-other");
        let mut seed: Vec<Write> = own
            .iter()
            .chain(&foreign)
            .map(|address| set_value(address, json!({"keep":true})))
            .collect();
        let queued = [
            "steer", "followUp", "write", "nextRun", "r3", "r5", "r6", "r8",
        ];
        seed.extend(queued.iter().map(|id| {
            set_value(
                &pending_entry(id),
                json!({"type":"custom","customType":"test"}),
            )
        }));
        let lane = json!({"currentOperationId":"op","lastOperationId":null,"inbox":[{"entryId":"steer","kind":"steer"},{"entryId":"nextRun","kind":"nextRun"}]});
        seed.push(set_value(&lane_state("main"), lane.clone()));
        seed.push(append_list(
            &pending_assistant_frames("op", "response"),
            json!({"type":"text_delta","delta":"partial"}),
        ));
        seed.push(append_list(
            &pending_assistant_frames("op", "stale"),
            json!("not this response"),
        ));
        seed.push(append_list(
            &pending_assistant_frames("op-other", "response"),
            json!("foreign operation"),
        ));
        commit(&session, seed).await;
        storage.clear_commit_attempts();
        let writes = session
            .mutate(
                move |reader, context| {
                    Box::pin(async move {
                        operation_cleanup_writes(reader, "op", &operation, context).await
                    })
                },
                background_context(),
            )
            .await
            .unwrap();
        assert!(storage.get_commit_attempts().is_empty());
        let mut expected: Vec<Write> = own.iter().map(delete_value).collect();
        if is_effect {
            expected.push(delete_list(&pending_assistant_frames("op", "response")));
        }
        if is_tools {
            expected.push(delete_value(&pending_entry("r6")));
        }
        assert_eq!(writes, expected, "phase {phase_name}");
        commit(&session, writes).await;
        for address in &own {
            assert!(session
                .get_value(address, background_context())
                .await
                .unwrap()
                .is_none());
        }
        for address in &foreign {
            assert!(session
                .get_value(address, background_context())
                .await
                .unwrap()
                .is_some());
        }
        for id in queued {
            assert_eq!(
                session
                    .get_value(&pending_entry(id), background_context())
                    .await
                    .unwrap()
                    .is_some(),
                !(is_tools && id == "r6")
            );
        }
        assert_eq!(
            session
                .get_value(&lane_state("main"), background_context())
                .await
                .unwrap()
                .unwrap()
                .value,
            lane
        );
        assert_eq!(
            session
                .read_list(
                    &pending_assistant_frames("op", "response"),
                    None,
                    background_context()
                )
                .await
                .unwrap()
                .len(),
            usize::from(!is_effect)
        );
        for (op, response) in [("op", "stale"), ("op-other", "response")] {
            assert_eq!(
                session
                    .read_list(
                        &pending_assistant_frames(op, response),
                        None,
                        background_context()
                    )
                    .await
                    .unwrap()
                    .len(),
                1
            );
        }
    }
}
#[tokio::test]
async fn cleanup_tool_outcomes_deduplicate_in_encounter_order_without_touching_placed_results() {
    let mut phase = phases().into_iter().find(|v| v["at"] == "tools").unwrap();
    phase["batch"]["calls"] = json!([
        {"sourceIndex":0,"resultEntryId":"z","status":"outcome_ready","terminate":false},
        {"sourceIndex":1,"resultEntryId":"placed","status":"completed","terminate":false},
        {"sourceIndex":2,"resultEntryId":"a","status":"outcome_ready","terminate":true},
        {"sourceIndex":3,"resultEntryId":"z","status":"outcome_ready","terminate":false}
    ]);
    let operation = serde_json::from_value(state(phase)).unwrap();
    let (session, _) = fixture();
    let writes = session
        .mutate(
            move |reader, context| {
                Box::pin(async move {
                    operation_cleanup_writes(reader, "tools", &operation, context).await
                })
            },
            background_context(),
        )
        .await
        .unwrap();
    assert_eq!(
        writes,
        vec![
            delete_value(&operation_meta("tools")),
            delete_value(&operation_state("tools")),
            delete_value(&pending_entry("z")),
            delete_value(&pending_entry("a"))
        ]
    );
}
#[test]
fn result_record_is_a_flat_snapshot_of_metadata_with_injected_completion_clock() {
    for intent in [
        json!({"kind":"run","promptEntryIds":["prompt"]}),
        json!({"kind":"compaction"}),
        json!({"kind":"navigation","targetId":null,"summarize":false}),
    ] {
        let meta: OperationMeta = serde_json::from_value(json!({"operationId":"run","lane":"main","sourceTipId":"source","startedAt":10,"intent":intent})).unwrap();
        let record = operation_result_record_at(
            &meta,
            TerminalStatus::Failed,
            Some("tip".into()),
            Some(OperationError {
                code: "provider".into(),
                message: "failed".into(),
                details: Some(Value::Null),
            }),
            20,
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(record).unwrap(),
            json!({"operationId":"run","kind":intent["kind"],"status":"failed","error":{"code":"provider","message":"failed","details":null},"fromTipId":"source","tipId":"tip","startedAt":10,"endedAt":20})
        );
    }
}
#[test]
fn result_record_enforces_error_presence_for_all_terminal_statuses() {
    let meta: OperationMeta = serde_json::from_value(json!({"operationId":"run","lane":"main","sourceTipId":null,"startedAt":1,"intent":{"kind":"run","promptEntryIds":[]}})).unwrap();
    for status in [
        TerminalStatus::Completed,
        TerminalStatus::Declined,
        TerminalStatus::Aborted,
        TerminalStatus::Failed,
    ] {
        for has_error in [false, true] {
            let error = has_error.then(|| OperationError {
                code: "x".into(),
                message: "x".into(),
                details: None,
            });
            let record = operation_result_record_at(&meta, status, None, error, 2);
            if (status == TerminalStatus::Failed) == has_error {
                assert!(record.is_ok());
            } else {
                let error = record.unwrap_err();
                assert!(error.downcast_ref::<SessionInvariantError>().is_some());
                assert_eq!(
                    error.to_string(),
                    "Only a failed operation result may carry an error"
                );
            }
        }
    }
    let before = crate::ai::now_ms();
    let record = operation_result_record(&meta, TerminalStatus::Completed, None, None).unwrap();
    assert!((before..=crate::ai::now_ms()).contains(&record.ended_at));
}
