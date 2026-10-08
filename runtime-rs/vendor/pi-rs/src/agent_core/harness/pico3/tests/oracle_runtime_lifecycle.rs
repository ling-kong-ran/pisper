//! Runtime registry and lifecycle oracles from spec-plugins-lifecycle.test.ts.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::support_runtime::*;
use crate::agent_core::harness::pico3::runtime::{
    abort_closure_from, AbortFn, Kind, PhaseFn, Step, TaskKind,
};
use crate::agent_core::harness::pico3::session::CreateTaskOptions;
use crate::agent_core::harness::pico3::types::{
    AnyKind, BasicKind, Completion, DocRef, KindConfig, TaskStatus,
};

fn abort() -> AbortFn {
    Arc::new(|_task, _rt, _ctx| {
        Box::pin(async {
            Ok(abort_closure_from(|_tx, _current, _ctx| {
                Box::pin(async { Ok(Value::Null) })
            }))
        })
    })
}

fn simple_kind(name: &str, result: &str, gate: Option<Gate>) -> Arc<dyn Kind> {
    let result = result.to_owned();
    let initial: PhaseFn = Arc::new(move |_task, _rt, ctx| {
        let gate = gate.clone();
        let result = result.clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                gate.wait(ctx).await?;
            }
            Ok(Step::Done(Box::new(move |_tx, _task, _ctx| {
                Box::pin(async move { Ok(Completion::completed(json!(result))) })
            })))
        })
    });
    Arc::new(TaskKind::new(name, initial, HashMap::new(), abort()))
}

async fn create(env: &Env, kind: Arc<dyn AnyKind>, after: Vec<i64>) -> anyhow::Result<i64> {
    env.root
        .commit(
            move |tx, _ctx| {
                Box::pin(async move {
                    Ok(tx
                        .create_task_kind(
                            &kind,
                            Value::Null,
                            CreateTaskOptions {
                                conversation_id: Some(1),
                                background: true,
                                after,
                            },
                        )?
                        .id)
                })
            },
            ctx(),
        )
        .await
}

async fn terminal(env: &Env, id: i64) -> crate::agent_core::harness::pico3::types::Task {
    tokio::time::timeout(Duration::from_secs(8), env.h.wait_for_task(id, ctx()))
        .await
        .expect("registered work must settle")
        .unwrap()
}

#[tokio::test]
async fn runtime_registration_seeds_defaults_into_already_loaded_documents() {
    let env = open(OpenOptions::default()).await.unwrap();
    env.root.rewindable(ctx()).await.unwrap();
    let initial: PhaseFn = Arc::new(|_task, rt, ctx| {
        Box::pin(async move {
            let observed = rt
                .commit(
                    |tx, task, _ctx| {
                        Box::pin(async move {
                            let facade = tx.config_get(task.conversation_id, "enabled")?;
                            let doc = tx.snapshot(DocRef::Rewindable {
                                conversation_id: task.conversation_id,
                            })?;
                            Ok(json!({ "facade": facade, "document": doc.get("enabled") }))
                        })
                    },
                    ctx,
                )
                .await?;
            Ok(Step::Done(Box::new(move |_tx, _task, _ctx| {
                Box::pin(async move { Ok(Completion::completed(observed)) })
            })))
        })
    });
    let kind = Arc::new(
        TaskKind::new("spec.runtime-default", initial, HashMap::new(), abort()).with_metadata(
            Arc::new(BasicKind::new("spec.runtime-default").config(KindConfig {
                rewindable: json!({ "enabled": false }).as_object().unwrap().clone(),
                ..Default::default()
            })),
        ),
    );
    let _unregister = env.h.register_task_kind(kind).unwrap();
    let id = create(
        &env,
        env.h.builtin_kind("spec.runtime-default").unwrap(),
        vec![],
    )
    .await
    .unwrap();
    assert_eq!(
        result_of(&terminal(&env, id).await),
        Some(json!({ "facade": false, "document": false }))
    );
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn running_kind_keeps_captured_config_authority_after_unregister() {
    let gate = Gate::new();
    let initial: PhaseFn = Arc::new({
        let gate = gate.clone();
        move |_task, rt, ctx| {
            let gate = gate.clone();
            Box::pin(async move {
                gate.wait(ctx.clone()).await?;
                let observed = rt
                    .commit(
                        |tx, task, _ctx| {
                            Box::pin(
                                async move { tx.config_get(task.conversation_id, "leaseValue") },
                            )
                        },
                        ctx,
                    )
                    .await?;
                Ok(Step::Done(Box::new(move |_tx, _task, _ctx| {
                    Box::pin(async move { Ok(Completion::completed(observed)) })
                })))
            })
        }
    });
    let kind = Arc::new(
        TaskKind::new("spec.active-reload", initial, HashMap::new(), abort()).with_metadata(
            Arc::new(
                BasicKind::new("spec.active-reload").config(KindConfig {
                    sticky: json!({ "leaseValue": "old-default" })
                        .as_object()
                        .unwrap()
                        .clone(),
                    ..Default::default()
                }),
            ),
        ),
    );
    let env = open(OpenOptions::default()).await.unwrap();
    let unregister = env.h.register_task_kind(kind).unwrap();
    let id = create(
        &env,
        env.h.builtin_kind("spec.active-reload").unwrap(),
        vec![],
    )
    .await
    .unwrap();
    gate.arrivals(1).await;
    unregister();
    gate.open();
    let task = terminal(&env, id).await;
    assert_eq!(outcome_status(&task), "completed");
    assert_eq!(result_of(&task), Some(json!("old-default")));
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn dependency_blocked_task_waits_for_replacement_kind() {
    let gate = Gate::new();
    let env = open(OpenOptions {
        task_kinds: vec![simple_kind("blocker", "done", Some(gate.clone()))],
        ..Default::default()
    })
    .await
    .unwrap();
    let original = simple_kind("reload", "original", None);
    let remove = env.h.register_task_kind(original).unwrap();
    let dependency = create(&env, env.h.builtin_kind("blocker").unwrap(), vec![])
        .await
        .unwrap();
    let target = create(
        &env,
        env.h.builtin_kind("reload").unwrap(),
        vec![dependency],
    )
    .await
    .unwrap();
    gate.arrivals(1).await;
    remove();
    gate.open();
    terminal(&env, dependency).await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(
        env.h
            .session()
            .storage()
            .task(target, ctx())
            .await
            .unwrap()
            .unwrap()
            .status,
        TaskStatus::Pending
    );
    let _unregister = env
        .h
        .register_task_kind(simple_kind("reload", "replacement", None))
        .unwrap();
    let task = terminal(&env, target).await;
    assert_eq!(outcome_status(&task), "completed");
    assert_eq!(result_of(&task), Some(json!("replacement")));
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn old_unsubscribe_is_idempotent_and_cannot_remove_a_replacement() {
    let env = open(OpenOptions::default()).await.unwrap();
    let old = simple_kind("spec.reload-kind", "old", None);
    let old_token = old.metadata();
    let remove_old = env.h.register_task_kind(old).unwrap();
    remove_old();
    remove_old();
    let _remove_current = env
        .h
        .register_task_kind(simple_kind("spec.reload-kind", "replacement", None))
        .unwrap();
    remove_old();
    let error = create(&env, old_token, vec![]).await.unwrap_err();
    assert!(
        error.to_string().contains("not the registered token"),
        "{error}"
    );
    let id = create(
        &env,
        env.h.builtin_kind("spec.reload-kind").unwrap(),
        vec![],
    )
    .await
    .unwrap();
    let task = terminal(&env, id).await;
    assert_eq!(outcome_status(&task), "completed");
    assert_eq!(result_of(&task), Some(json!("replacement")));
    env.close(ctx()).await.unwrap();
}

#[tokio::test]
async fn custom_task_describe_does_not_receive_private_slot_memos() {
    use crate::agent_core::harness::pico3::session::TaskRef;
    let gate = Gate::new();
    let metadata: Arc<dyn AnyKind> = Arc::new(
        BasicKind::new("spec.private-memo-view")
            .slot(|_| json!({"progress":0}).as_object().unwrap().clone())
            .describe(|_task, slot| Ok(slot.cloned().map(Value::Object).unwrap_or(Value::Null))),
    );
    let initial: PhaseFn = Arc::new({
        let gate = gate.clone();
        let metadata = metadata.clone();
        move |task, rt, context| {
            let gate = gate.clone();
            let metadata = metadata.clone();
            Box::pin(async move {
                rt.commit(
                    move |tx, _current, _ctx| {
                        Box::pin(async move {
                            tx.slot_update(
                                &TaskRef {
                                    id: task.id,
                                    kind: metadata,
                                },
                                |slot| {
                                    slot["progress"] = json!(1);
                                    slot["memos"] = json!({"secret":"never-project"});
                                },
                            )?;
                            Ok(())
                        })
                    },
                    context.clone(),
                )
                .await?;
                gate.wait(context).await?;
                Ok(Step::Done(Box::new(|_tx, _task, _ctx| {
                    Box::pin(async { Ok(Completion::completed(Value::Null)) })
                })))
            })
        }
    });
    let kind = Arc::new(
        TaskKind::new("spec.private-memo-view", initial, HashMap::new(), abort())
            .with_metadata(metadata.clone()),
    );
    let env = open(OpenOptions {
        task_kinds: vec![kind],
        ..Default::default()
    })
    .await
    .unwrap();
    let id = create(&env, metadata, vec![]).await.unwrap();
    tokio::time::timeout(Duration::from_secs(8), gate.arrivals(1))
        .await
        .expect("initial must reach gate");
    let watch = env.root.watch(ctx()).await.unwrap();
    assert_eq!(
        watch.view.tasks[&id.to_string()].status,
        json!({"progress":1})
    );
    assert!(!serde_json::to_string(&watch.view)
        .unwrap()
        .contains("never-project"));
    let sticky = env.root.sticky(ctx()).await.unwrap();
    assert_eq!(
        sticky["tasks"][id.to_string()]["memos"]["secret"],
        "never-project",
        "projection must not mutate stored state"
    );
    watch.stop();
    gate.open();
    terminal(&env, id).await;
    env.close(ctx()).await.unwrap();
}
