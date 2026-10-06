//! Real-runtime capability and lifetime oracles. Source: harness.ts and
//! spec-context-capabilities.test.ts; unlike direct Tx probes these exercise
//! the runtime-to-scheduler forwarding boundary.

use super::support_runtime::*;
use crate::agent_core::harness::pico3::runtime::{
    abort_closure_from, AbortFn, ChildConversation, Kind, Next, PhaseFn, Runtime, Step, TaskKind,
    ToolApi,
};
use crate::agent_core::harness::pico3::session::CreateTaskOptions;
use crate::agent_core::harness::pico3::types::{
    is_named, AnyKind, Completion, ConversationSpec, OwnedConversationSpec, FORBIDDEN,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn abort() -> AbortFn {
    Arc::new(|_, _, _| {
        Box::pin(async {
            Ok(abort_closure_from(|_, _, _| {
                Box::pin(async { Ok(Value::Null) })
            }))
        })
    })
}
fn done(value: Value) -> Step {
    Step::Done(Box::new(move |_, _, _| {
        Box::pin(async move { Ok(Completion::completed(value)) })
    }))
}
fn task_kind(name: &str, initial: PhaseFn) -> Arc<dyn Kind> {
    Arc::new(TaskKind::new(name, initial, HashMap::new(), abort()))
}
async fn create(env: &Env, kind: Arc<dyn AnyKind>, conversation: i64, input: Value) -> i64 {
    let handle = env
        .h
        .conversation(conversation, ctx())
        .await
        .unwrap()
        .unwrap();
    handle
        .commit(
            move |tx, _| {
                Box::pin(async move {
                    Ok(tx
                        .create_task_kind(
                            &kind,
                            input,
                            CreateTaskOptions {
                                conversation_id: Some(conversation),
                                background: true,
                                after: vec![],
                            },
                        )?
                        .id)
                })
            },
            ctx(),
        )
        .await
        .unwrap()
}
async fn terminal(env: &Env, id: i64) -> crate::agent_core::harness::pico3::types::Task {
    tokio::time::timeout(Duration::from_secs(8), env.h.wait_for_task(id, ctx()))
        .await
        .expect("task must settle")
        .unwrap()
}
async fn arrived(gate: &Gate) {
    tokio::time::timeout(Duration::from_secs(8), gate.arrivals(1))
        .await
        .expect("task must enter phase");
}
fn forbidden<T>(result: anyhow::Result<T>) -> bool {
    result
        .err()
        .is_some_and(|error| is_named(&error, FORBIDDEN))
}

#[tokio::test]
async fn runtime_abort_rejects_foreign_live_and_retained_tasks_and_unowned_conversations() {
    let gate = Gate::new();
    let worker = task_kind(
        "scope.worker",
        Arc::new({
            let gate = gate.clone();
            move |task, _, context| {
                let gate = gate.clone();
                Box::pin(async move {
                    if task.input == json!("wait") {
                        gate.wait(context).await?;
                    }
                    Ok(done(Value::Null))
                })
            }
        }),
    );
    let probe = task_kind(
        "scope.probe",
        Arc::new(move |task, rt, context| {
            Box::pin(async move {
                let foreign = task.input["conversation"].as_i64().unwrap();
                let live = task.input["live"].as_i64().unwrap();
                let retained = task.input["retained"].as_i64().unwrap();
                let results = vec![
                    forbidden(rt.abort_task(live, context.clone()).await),
                    forbidden(rt.abort_task(retained, context.clone()).await),
                    forbidden(rt.abort_conversation(foreign, context.clone()).await),
                    forbidden(
                        rt.abort_conversation(rt.conversation_id(), context.clone())
                            .await,
                    ),
                    rt.abort_task(i64::MAX, context)
                        .await
                        .err()
                        .is_some_and(|e| e.to_string().contains("not found")),
                ];
                Ok(done(json!(results)))
            })
        }),
    );
    let worker_meta = worker.metadata();
    let probe_meta = probe.metadata();
    let env = open(OpenOptions {
        task_kinds: vec![worker, probe],
        ..Default::default()
    })
    .await
    .unwrap();
    let foreign = env
        .h
        .create_conversation(ConversationSpec::default(), None, ctx())
        .await
        .unwrap();
    let retained = create(&env, worker_meta.clone(), foreign.id, Value::Null).await;
    assert_eq!(outcome_status(&terminal(&env, retained).await), "completed");
    let live = create(&env, worker_meta, foreign.id, json!("wait")).await;
    arrived(&gate).await;
    let id = create(
        &env,
        probe_meta,
        1,
        json!({"conversation":foreign.id,"live":live,"retained":retained}),
    )
    .await;
    let result = terminal(&env, id).await;
    assert_eq!(outcome_status(&result), "completed", "{result:?}");
    assert_eq!(
        result_of(&result),
        Some(json!([true, true, true, true, true]))
    );
    assert_ne!(
        env.storage.task(live, ctx()).await.unwrap().unwrap().abort,
        Some(true)
    );
    gate.open();
    terminal(&env, live).await;
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn runtime_can_abort_same_conversation_tasks_and_its_owned_subtree() {
    let gate = Gate::new();
    let worker = task_kind(
        "scope.allowed-worker",
        Arc::new({
            let gate = gate.clone();
            move |_, _, context| {
                let gate = gate.clone();
                Box::pin(async move {
                    gate.wait(context).await?;
                    Ok(done(Value::Null))
                })
            }
        }),
    );
    let worker_meta = worker.metadata();
    let probe = task_kind(
        "scope.allowed-probe",
        Arc::new({
            let worker_meta = worker_meta.clone();
            move |task, rt, context| {
                let worker_meta = worker_meta.clone();
                Box::pin(async move {
                    let sibling = task.input.as_i64().unwrap();
                    rt.abort_task(sibling, context.clone()).await?;
                    let child = rt
                        .create_owned_conversation(
                            OwnedConversationSpec::default(),
                            context.clone(),
                        )
                        .await?;
                    let grandchild = rt
                        .commit(
                            move |tx, _, _| {
                                Box::pin(async move {
                                    use crate::agent_core::harness::pico3::types::{
                                        ConversationParentSpec, ParentAt,
                                    };
                                    tx.create_conversation(&ConversationSpec {
                                        parent: Some(ConversationParentSpec::Parent {
                                            conversation_id: child,
                                            at: ParentAt::Start,
                                        }),
                                        ..Default::default()
                                    })
                                })
                            },
                            context.clone(),
                        )
                        .await?;
                    let nested_task = rt
                        .commit(
                            move |tx, _, _| {
                                Box::pin(async move {
                                    Ok(tx
                                        .create_task_kind(
                                            &worker_meta,
                                            Value::Null,
                                            CreateTaskOptions {
                                                conversation_id: Some(grandchild),
                                                background: true,
                                                after: vec![],
                                            },
                                        )?
                                        .id)
                                })
                            },
                            context.clone(),
                        )
                        .await?;
                    rt.abort_task(nested_task, context.clone()).await?;
                    rt.abort_conversation(grandchild, context.clone()).await?;
                    rt.abort_conversation(child, context).await?;
                    Ok(done(
                        json!({"child":child,"grandchild":grandchild,"nested":nested_task}),
                    ))
                })
            }
        }),
    );
    let probe_meta = probe.metadata();
    let env = open(OpenOptions {
        task_kinds: vec![worker, probe],
        ..Default::default()
    })
    .await
    .unwrap();
    let sibling = create(&env, worker_meta, 1, Value::Null).await;
    arrived(&gate).await;
    let id = create(&env, probe_meta, 1, json!(sibling)).await;
    let result = terminal(&env, id).await;
    assert_eq!(outcome_status(&result), "completed", "{result:?}");
    assert_eq!(outcome_status(&terminal(&env, sibling).await), "aborted");
    let nested = result_of(&result).unwrap()["nested"].as_i64().unwrap();
    assert_eq!(outcome_status(&terminal(&env, nested).await), "aborted");
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn captured_runtime_expires_at_phase_return_for_commits_and_scheduler_operations() {
    let captured: Arc<Mutex<Option<Arc<Runtime>>>> = Arc::new(Mutex::new(None));
    let owned = Arc::new(Mutex::new(0));
    let phase_gate = Gate::new();
    let initial: PhaseFn = Arc::new({
        let captured = captured.clone();
        let owned = owned.clone();
        move |_, rt, context| {
            let captured = captured.clone();
            let owned = owned.clone();
            Box::pin(async move {
                *owned.lock().unwrap() = rt
                    .create_owned_conversation(OwnedConversationSpec::default(), context)
                    .await?;
                *captured.lock().unwrap() = Some(rt);
                Ok(Step::Next(Next::Checkpoint(
                    json!({"phase":"later"}).as_object().unwrap().clone(),
                )))
            })
        }
    });
    let later: PhaseFn = Arc::new({
        let gate = phase_gate.clone();
        move |_, _, context| {
            let gate = gate.clone();
            Box::pin(async move {
                gate.wait(context).await?;
                Ok(done(Value::Null))
            })
        }
    });
    let kind: Arc<dyn Kind> = Arc::new(TaskKind::new(
        "scope.phase-lifetime",
        initial,
        HashMap::from([("later".to_owned(), later)]),
        abort(),
    ));
    let metadata = kind.metadata();
    let env = open(OpenOptions {
        task_kinds: vec![kind],
        ..Default::default()
    })
    .await
    .unwrap();
    let id = create(&env, metadata, 1, Value::Null).await;
    arrived(&phase_gate).await;
    let rt = captured.lock().unwrap().clone().unwrap();
    let child = *owned.lock().unwrap();
    assert!(forbidden(
        rt.commit(|_, _, _| Box::pin(async { Ok(()) }), ctx()).await
    ));
    assert!(forbidden(rt.abort_task(id, ctx()).await));
    assert!(forbidden(rt.abort_conversation(child, ctx()).await));
    assert!(forbidden(
        rt.create_owned_conversation(OwnedConversationSpec::default(), ctx())
            .await
    ));
    phase_gate.open();
    assert_eq!(outcome_status(&terminal(&env, id).await), "completed");
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn captured_child_handle_abort_obeys_invocation_lifetime() {
    let captured: Arc<Mutex<Option<ChildConversation>>> = Arc::new(Mutex::new(None));
    let tool = Arc::new(
        crate::agent_core::harness::pico3::runtime::ToolDeclaration {
            name: "child-lifetime".to_owned(),
            description: String::new(),
            parameters: json!({"type":"object"}),
            replay: None,
            output: None,
            execute: Arc::new({
                let captured = captured.clone();
                move |_, api, context| {
                    let captured = captured.clone();
                    Box::pin(async move {
                        let child = api
                            .conversation(OwnedConversationSpec::default(), context)
                            .await?;
                        *captured.lock().unwrap() = Some(child);
                        Ok(Default::default())
                    })
                }
            }),
        },
    );
    let env = open(OpenOptions {
        tools: vec![tool],
        ..Default::default()
    })
    .await
    .unwrap();
    use crate::agent_core::harness::pico3::types::{SendInput, UserInput};
    let input = env
        .root
        .send(
            SendInput {
                content: UserInput::Text("tool:child-lifetime".to_owned()),
                ..Default::default()
            },
            ctx(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(8), input.wait(ctx()))
        .await
        .unwrap()
        .unwrap();
    let child = captured.lock().unwrap().take().unwrap();
    assert!(forbidden(child.abort(ctx()).await));
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn base_task_api_rejects_invocation_only_stream_progress_and_memo() {
    let kind = task_kind(
        "scope.base-tool-api",
        Arc::new(|task, rt, context| {
            Box::pin(async move {
                let api = ToolApi::base(rt, task.id, task.conversation_id);
                let progress = api.progress(|_| {}, context.clone()).await.is_err();
                let memo = api.memo("outside", None, context).await.is_err();
                let stream = api
                    .stream(
                        crate::agent_core::harness::pico3::runtime::StreamChunk::Text(
                            "outside".to_owned(),
                        ),
                    )
                    .is_err();
                Ok(done(json!([api.can_stream(), progress, memo, stream])))
            })
        }),
    );
    let metadata = kind.metadata();
    let env = open(OpenOptions {
        task_kinds: vec![kind],
        ..Default::default()
    })
    .await
    .unwrap();
    let id = create(&env, metadata, 1, Value::Null).await;
    assert_eq!(
        result_of(&terminal(&env, id).await),
        Some(json!([false, true, true, true]))
    );
    env.close(ctx()).await.unwrap();
}
